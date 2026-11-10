//! zetwerk command line.

mod bridge;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use lexopt::prelude::*;
use zetwerk_app::app::{self, Handle, Request};
use zetwerk_app::keys::{self, KeyPress};
use zetwerk_app::power::PowerMode;
use zetwerk_mcp::Output;
use zetwerk_typst::{Environment, FontSet, Project};

use crate::bridge::Bridge;

const USAGE: &str = "\
Usage: zetwerk <file.typ> [options]
       zetwerk render <file.typ> [options]
       zetwerk mcp

Opens a Typst document, or with `render` writes one page of it as a PNG.
`zetwerk mcp` connects an AI agent to the open window: it is an MCP server
on standard input and output, to be started by the agent
(for Claude Code: `claude mcp add zetwerk -- zetwerk mcp`).

Options:
      --power <mode>      auto (default), saver or performance
      --embedded-fonts    Use only the fonts shipped with Typst
  -h, --help              Show this help

Options for `render`:
  -o, --output <path>     Where to write the PNG (default: <file>.png)
  -p, --page <n>          Page to render, starting at 1 (default: 1)
  -s, --scale <n>         Pixels per point (default: 2)

Options for developing zetwerk itself:
      --dev               Also offer agents tools to press keys in the
                          window and to capture it
      --keys <keys>       Press these keys once the document is shown,
                          in Vim notation, e.g. 'jj<C-d>:perf<CR>'
      --screenshot <path> Then save the window as a PNG and quit
  zetwerk call <tool> [json-arguments] [-o <path>]
                          Call a tool of the open window, as an agent
                          would; images are written to <path>";

/// How long a scripted run waits for the window before giving up.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Default, PartialEq)]
enum Command {
    #[default]
    Open,
    Render,
    Mcp,
    Call,
}

#[derive(Default)]
struct Args {
    command: Command,
    /// Positional arguments after the command.
    values: Vec<OsString>,
    dev: bool,
    output: Option<PathBuf>,
    page: Option<usize>,
    scale: Option<f64>,
    embedded_fonts: bool,
    power: PowerMode,
    keys: Vec<KeyPress>,
    screenshot: Option<PathBuf>,
}

fn run() -> Result<(), String> {
    let Some(args) = parse_args()? else {
        println!("{USAGE}");
        return Ok(());
    };
    match args.command {
        Command::Mcp => return mcp(),
        Command::Call => return call(&args),
        Command::Open | Command::Render => {}
    }
    let input = match args.values.as_slice() {
        [input] => PathBuf::from(input),
        [] => return Err(format!("missing input file\n\n{USAGE}")),
        [_, extra, ..] => return Err(format!("unexpected argument {}", extra.display())),
    };
    let font_set = if args.embedded_fonts {
        FontSet::Embedded
    } else {
        FontSet::EmbeddedAndSystem
    };
    if args.command == Command::Render {
        return render(&args, &input, font_set);
    }
    if args.output.is_some() || args.page.is_some() || args.scale.is_some() {
        return Err("--output, --page and --scale belong to `zetwerk render`".to_owned());
    }
    if !input.is_file() {
        return Err(format!("{} is not a file", input.display()));
    }

    let (script_result, script_done) = mpsc::channel();
    let script = (!args.keys.is_empty() || args.screenshot.is_some())
        .then(|| (args.keys.clone(), args.screenshot.clone()));
    let options = app::Options {
        path: input,
        font_set,
        power: args.power,
    };
    // Kept until the window closes; dropping it stops the server.
    let mut server = None;
    app::run(options, |handle| {
        // Without a runtime directory, or if the socket cannot be set up,
        // the window works as usual; agents just cannot reach it.
        server = zetwerk_mcp::socket_path().and_then(|socket| {
            let editor = Arc::new(Bridge::new(handle.clone()));
            zetwerk_mcp::serve(&socket, editor, args.dev).ok()
        });
        if let Some((keys, screenshot)) = script {
            std::thread::spawn(move || {
                let result = run_script(&handle, keys, screenshot.as_deref());
                let _ = script_result.send(result);
                handle.send(Request::Quit);
            });
        }
    })?;
    // No script ran, or it ended with the window: either way its result
    // (if any) is in the channel by now.
    script_done.try_recv().unwrap_or(Ok(()))
}

fn parse_args() -> Result<Option<Args>, String> {
    let fail = |err: lexopt::Error| err.to_string();
    let mut parser = lexopt::Parser::from_env();
    let mut args = Args::default();
    while let Some(arg) = parser.next().map_err(fail)? {
        match arg {
            Short('h') | Long("help") => return Ok(None),
            Short('o') | Long("output") => {
                args.output = Some(parser.value().map_err(fail)?.into());
            }
            Short('p') | Long("page") => {
                args.page = Some(parser.value().and_then(|v| v.parse()).map_err(fail)?);
            }
            Short('s') | Long("scale") => {
                args.scale = Some(parser.value().and_then(|v| v.parse()).map_err(fail)?);
            }
            Long("embedded-fonts") => args.embedded_fonts = true,
            Long("dev") => args.dev = true,
            Long("power") => {
                let name = parser
                    .value()
                    .and_then(lexopt::ValueExt::string)
                    .map_err(fail)?;
                args.power = PowerMode::from_name(&name)
                    .ok_or_else(|| format!("unknown power mode {name:?}"))?;
            }
            Long("keys") => {
                let notation = parser
                    .value()
                    .and_then(lexopt::ValueExt::string)
                    .map_err(fail)?;
                args.keys = keys::parse(&notation)?;
            }
            Long("screenshot") => args.screenshot = Some(parser.value().map_err(fail)?.into()),
            Value(value) if args.command == Command::Open && args.values.is_empty() => {
                match value.to_str() {
                    Some("render") => args.command = Command::Render,
                    Some("mcp") => args.command = Command::Mcp,
                    Some("call") => args.command = Command::Call,
                    _ => args.values.push(value),
                }
            }
            Value(value) => args.values.push(value),
            other => return Err(fail(other.unexpected())),
        }
    }
    Ok(Some(args))
}

/// Drives an open window: waits until the document is shown, presses
/// `keys`, and saves a screenshot of the result.
fn run_script(
    handle: &Handle,
    keys: Vec<KeyPress>,
    screenshot: Option<&Path>,
) -> Result<(), String> {
    let gone = || "the window closed before the script finished".to_owned();
    let timeout = |_| format!("the window did not settle within {SCRIPT_TIMEOUT:?}");

    let (settled, when_settled) = mpsc::channel();
    handle
        .send(Request::WhenSettled(settled))
        .then_some(())
        .ok_or_else(gone)?;
    when_settled.recv_timeout(SCRIPT_TIMEOUT).map_err(timeout)?;
    handle
        .send(Request::Keys(keys))
        .then_some(())
        .ok_or_else(gone)?;

    let Some(path) = screenshot else {
        return Ok(());
    };
    let (reply, shot) = mpsc::channel();
    handle
        .send(Request::Screenshot(reply))
        .then_some(())
        .ok_or_else(gone)?;
    let png = bridge::encode(shot.recv_timeout(SCRIPT_TIMEOUT).map_err(timeout)??)?;
    std::fs::write(path, png).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

/// `zetwerk mcp`: forwards an agent's standard input and output to the
/// window's MCP server.
fn mcp() -> Result<(), String> {
    let socket = zetwerk_mcp::find_socket()?;
    zetwerk_mcp::proxy_stdio(&socket)
        .map_err(|err| format!("lost the connection to the zetwerk window: {err}"))
}

/// `zetwerk call`: calls one tool of the window and prints its answer.
fn call(args: &Args) -> Result<(), String> {
    let (tool, arguments) = match args.values.as_slice() {
        [tool] => (tool, serde_json::Map::new()),
        [tool, json] => {
            let json = json.to_str().ok_or("the arguments are not valid UTF-8")?;
            let value = serde_json::from_str(json)
                .map_err(|err| format!("the arguments are not a JSON object: {err}"))?;
            (tool, value)
        }
        _ => return Err("usage: zetwerk call <tool> [json-arguments] [-o <path>]".to_owned()),
    };
    let tool = tool.to_str().ok_or("the tool name is not valid UTF-8")?;
    let socket = zetwerk_mcp::find_socket()?;
    for output in zetwerk_mcp::call(&socket, tool, arguments)? {
        match (output, &args.output) {
            (Output::Text(text), _) => println!("{text}"),
            (Output::Image { data, .. }, Some(path)) => {
                std::fs::write(path, data)
                    .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
                println!("{}", path.display());
            }
            (Output::Image { mime_type, data }, None) => {
                println!(
                    "[{mime_type}, {} bytes; pass -o <path> to save it]",
                    data.len()
                );
            }
        }
    }
    Ok(())
}

fn render(args: &Args, input: &Path, font_set: FontSet) -> Result<(), String> {
    let page_number = args.page.unwrap_or(1);
    let scale = args.scale.unwrap_or(2.0);
    if page_number == 0 {
        return Err("pages are numbered from 1".to_owned());
    }
    if !(scale.is_finite() && scale > 0.0 && scale <= 16.0) {
        return Err("scale must be between 0 and 16".to_owned());
    }
    let output = args.output.clone().unwrap_or_else(|| {
        let mut name = OsString::from(input.as_os_str());
        name.push(".png");
        name.into()
    });

    let project =
        Project::open(Environment::new(font_set), input).map_err(|err| err.to_string())?;
    let world = project.snapshot();
    let compiled = zetwerk_typst::compile(&world);
    for warning in &compiled.warnings {
        eprintln!("{}", zetwerk_typst::describe(&world, warning));
    }
    if compiled.panicked {
        return Err("the Typst compiler crashed on this document".to_owned());
    }
    let Some(document) = compiled.document else {
        let describe = |error| zetwerk_typst::describe(&world, error);
        return Err(compiled
            .errors
            .iter()
            .map(describe)
            .collect::<Vec<_>>()
            .join("\n"));
    };
    let pages = document.pages();
    let page = pages.get(page_number - 1).ok_or_else(|| {
        format!(
            "page {page_number} does not exist; the document has {} page(s)",
            pages.len()
        )
    })?;
    zetwerk_render::render_page(page, scale)
        .save_png(&output)
        .map_err(|err| format!("cannot write {}: {err}", output.display()))?;
    println!(
        "{} (page {page_number} of {})",
        output.display(),
        pages.len()
    );
    Ok(())
}
