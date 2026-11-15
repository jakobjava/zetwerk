//! zetwerk command line.

mod bridge;
mod config;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use lexopt::prelude::*;
use zetwerk_app::app::{self, Handle, Request};
use zetwerk_app::headless;
use zetwerk_app::keys::{self, KeyPress};
use zetwerk_app::power::PowerMode;
use zetwerk_mcp::Output;
use zetwerk_typst::{Environment, FontSet, Project};

use crate::bridge::Bridge;

const USAGE: &str = "\
Usage: zetwerk <file.typ> [options]
       zetwerk [directory] [options]
       zetwerk render <file.typ> [options]
       zetwerk mcp [file.typ]

Opens a Typst document, or with `render` writes one page of it as a PNG.
`zetwerk mcp` connects an AI agent to the open window: it is an MCP server
on standard input and output, to be started by the agent
(for Claude Code: `claude mcp add zetwerk -- zetwerk mcp`). With a file it
works on that document without a window, unless a window has it open.

Settings are read from ~/.config/zetwerk/config.toml:
  [ai]
  edit_mode = \"live\"      or \"propose\": agents' changes wait for you
  save = false            whether agents may write the file (unset: only
                          where no window shows it)
  [view]
  layout = \"split\"        or \"source\", \"page\"
  slides = false          one page at a time, whole
  ui_scale = 1.0          how large the status line, the lists and the
                          source are; :ui 1.5, :ui + and :ui - while open
  [power]
  mode = \"auto\"           or \"saver\", \"performance\"
  [language]
  spelling = true         mark words no Hunspell dictionary knows, in the
                          document's language (English, British first,
                          where it names none); ]s [s z= zg zw, :spell
  grammar = true          and what a grammar checker finds; :grammar
  language = \"nl\"         check as Dutch whatever the document says
  dictionaries = [\"~/d\"]  more directories with .aff and .dic files
  languagetool = \"http://localhost:8081\"
                          a LanguageTool server of yours for the grammar
                          of German, Dutch and English; without one,
                          English alone is checked, by built-in rules
  allow_remote = false    only this machine is sent text, unless true
  [[insert]]              your own things for the # menu, as many
  name = \"diagram\"        as you like: for a package, a theorem
  body = \"#diagram(|)\"    (| is where the cursor goes)
  block = true            it stands by itself, like a figure
  [keys.normal]           your own keys, as Vim's nnoremap
  \"<C-s>\" = \":w<CR>\"
  [keys.insert]           and inoremap (one key each)
  \"<C-s>\" = \"<Esc>:w<CR>\"

A file that another file includes is opened as part of that document:
the whole document is shown, the file is what is edited.

Without a file (or with a directory) the documents opened last are
listed, and new ones are begun there: :new <name> makes one, :recent
lists them again. A new document begins as an article, a report, a
letter, slides, notes, or as your own: .typ files in
~/.config/zetwerk/templates, where ‹what is between these› is a place to
fill in (Tab leads from one to the next).

Options:
      --main <file.typ>   The main file of the document, where zetwerk
                          does not find it itself
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
      --paced             Press the keys one by one, each once the one
                          before has been typeset, as somebody typing
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
    power: Option<PowerMode>,
    keys: Vec<KeyPress>,
    screenshot: Option<PathBuf>,
    /// Whether the keys of the script wait for each other's effect.
    paced: bool,
    main: Option<PathBuf>,
}

/// Makes sure there is a file at `path` to open: one that does not
/// exist yet is begun, empty. Nothing that is there is ever replaced.
fn begin_if_new(path: &Path) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    // A directory is where new documents are begun (see `app::Options`).
    if path.is_dir() {
        return Ok(());
    }
    if path.exists() {
        return Err(format!("{} is not a file", path.display()));
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|err| format!("cannot begin {}: {err}", path.display()))
}

/// What a document is opened with: what the command line says, then what
/// the settings say.
fn options(
    args: &Args,
    config: &config::Config,
    path: PathBuf,
    font_set: FontSet,
    complaint: Option<String>,
) -> Result<app::Options, String> {
    let (keys, odd_key) = config.keys.keymap();
    Ok(app::Options {
        main: main_of(&path, args.main.as_deref()),
        path,
        font_set,
        power: args
            .power
            .or(config.power.mode.map(Into::into))
            .unwrap_or_default(),
        state_dir: state_dir()?,
        agent_edits: config.ai.edit_mode.into(),
        complaint: complaint.or(odd_key),
        keys,
        layout: config.view.layout.map(Into::into),
        slides: config.view.slides,
        ui_scale: config.view.ui_scale,
        inserts: config.inserts(),
        language: config.language.settings(),
        templates: config::templates(),
    })
}

/// The main file of the document `file` is part of, if that is another
/// file: the one the user named, or the one that includes it.
fn main_of(file: &Path, named: Option<&Path>) -> Option<PathBuf> {
    // A directory is no document: nothing to look for.
    if file.is_dir() {
        return None;
    }
    let main = named.map_or_else(|| zetwerk_typst::find_main(file), Path::to_owned);
    let same = match (main.canonicalize(), file.canonicalize()) {
        (Ok(main), Ok(file)) => main == file,
        _ => main == file,
    };
    (!same).then_some(main)
}

fn run() -> Result<(), String> {
    let Some(args) = parse_args()? else {
        println!("{USAGE}");
        return Ok(());
    };
    match args.command {
        Command::Mcp => return mcp(&args),
        Command::Call => return call(&args),
        Command::Open | Command::Render => {}
    }
    let input = input(&args)?;
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
    begin_if_new(&input)?;

    let (script_result, script_done) = mpsc::channel();
    let paced = args.paced;
    let script = (!args.keys.is_empty() || args.screenshot.is_some())
        .then(|| (args.keys.clone(), args.screenshot.clone()));
    let (config, complaint) = config::load();
    let options = options(&args, &config, input, font_set, complaint)?;
    // Kept until the window closes; dropping it stops the server.
    let mut server = None;
    app::run(options, |handle| {
        // Without a runtime directory, or if the socket cannot be set up,
        // the window works as usual; agents just cannot reach it.
        server = zetwerk_mcp::socket_path().and_then(|socket| {
            let save = agents_save(&config.ai, true);
            let editor = Arc::new(Bridge::new(handle.clone()).offering(true, save));
            let tools = zetwerk_mcp::Tools {
                save,
                development: args.dev,
                headless: false,
            };
            zetwerk_mcp::serve(&socket, editor, tools).ok()
        });
        if let Some((keys, screenshot)) = script {
            std::thread::spawn(move || {
                let result = run_script(&handle, keys, screenshot.as_deref(), paced);
                let _ = script_result.send(result);
                handle.send(Request::Quit);
            });
        }
    })?;
    // No script ran, or it ended with the window: either way its result
    // (if any) is in the channel by now.
    script_done.try_recv().unwrap_or(Ok(()))
}

/// The file to open or render: the one named; to open, where none is,
/// the current directory, for the list of the documents opened last.
fn input(args: &Args) -> Result<PathBuf, String> {
    match args.values.as_slice() {
        [input] => Ok(PathBuf::from(input)),
        [] if args.command == Command::Open => Ok(PathBuf::from(".")),
        [] => Err(format!("missing input file\n\n{USAGE}")),
        [_, extra, ..] => Err(format!("unexpected argument {}", extra.display())),
    }
}

fn parse_args() -> Result<Option<Args>, String> {
    parse(lexopt::Parser::from_env())
}

/// The command line `parser` reads, or `None` where it asks for help.
fn parse(mut parser: lexopt::Parser) -> Result<Option<Args>, String> {
    let fail = |err: lexopt::Error| err.to_string();
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
                let mode = PowerMode::from_name(&name)
                    .ok_or_else(|| format!("unknown power mode {name:?}"))?;
                args.power = Some(mode);
            }
            Long("keys") => {
                let notation = parser
                    .value()
                    .and_then(lexopt::ValueExt::string)
                    .map_err(fail)?;
                args.keys = keys::parse(&notation)?;
            }
            Long("screenshot") => args.screenshot = Some(parser.value().map_err(fail)?.into()),
            Long("paced") => args.paced = true,
            Long("main") => args.main = Some(parser.value().map_err(fail)?.into()),
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
    paced: bool,
) -> Result<(), String> {
    let gone = || "the window closed before the script finished".to_owned();
    let timeout = |_| format!("the window did not settle within {SCRIPT_TIMEOUT:?}");

    let (settled, when_settled) = mpsc::channel();
    handle
        .send(Request::WhenSettled(settled))
        .then_some(())
        .ok_or_else(gone)?;
    when_settled.recv_timeout(SCRIPT_TIMEOUT).map_err(timeout)?;
    if paced {
        // As somebody types: every key sees what the one before led to.
        for key in keys {
            handle
                .send(Request::Keys(vec![key]))
                .then_some(())
                .ok_or_else(gone)?;
            let (settled, when_settled) = mpsc::channel();
            handle
                .send(Request::WhenSettled(settled))
                .then_some(())
                .ok_or_else(gone)?;
            when_settled.recv_timeout(SCRIPT_TIMEOUT).map_err(timeout)?;
        }
    } else {
        handle
            .send(Request::Keys(keys))
            .then_some(())
            .ok_or_else(gone)?;
    }

    let Some(path) = screenshot else {
        return Ok(());
    };
    // Of what the keys led to, once that is typeset.
    let (settled, when_settled) = mpsc::channel();
    handle
        .send(Request::WhenSettled(settled))
        .then_some(())
        .ok_or_else(gone)?;
    when_settled.recv_timeout(SCRIPT_TIMEOUT).map_err(timeout)?;
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
fn mcp(args: &Args) -> Result<(), String> {
    let proxy = |socket: PathBuf| {
        zetwerk_mcp::proxy_stdio(&socket)
            .map_err(|err| format!("lost the connection to the zetwerk window: {err}"))
    };
    let path = match args.values.as_slice() {
        [] => {
            let socket = zetwerk_mcp::find_socket().map_err(|why| {
                format!("{why}; `zetwerk mcp <file.typ>` works on a document without one")
            })?;
            return proxy(socket);
        }
        [path] => PathBuf::from(path),
        [_, extra, ..] => return Err(format!("unexpected argument {}", extra.display())),
    };
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    // Standard output is the agent's line from here on: complaints go to
    // standard error.
    let (config, complaint) = config::load();
    if let Some(complaint) = &complaint {
        eprintln!("{complaint}");
    }
    let font_set = if args.embedded_fonts {
        FontSet::Embedded
    } else {
        FontSet::EmbeddedAndSystem
    };
    let options = options(args, &config, path, font_set, None)?;
    match headless::spawn(&options) {
        Ok((handle, thread)) => {
            // There is no window to save in: the agent may, unless the
            // user said agents may not.
            let save = agents_save(&config.ai, false);
            let editor = Arc::new(Bridge::new(handle.clone()).offering(false, save));
            let tools = zetwerk_mcp::Tools {
                save,
                development: false,
                headless: true,
            };
            let served = zetwerk_mcp::serve_stdio(editor, tools);
            handle.send(Request::Quit);
            let _ = thread.join();
            served.map_err(|err| format!("the connection to the agent failed: {err}"))
        }
        // A window has the document: the agent works in that, and in no
        // other window.
        Err(headless::OpenError::AlreadyOpen) => {
            let socket = zetwerk_mcp::find_socket_of(&options.path).map_err(|why| {
                format!(
                    "{} is open in another zetwerk that cannot be reached: {why}",
                    options.path.display()
                )
            })?;
            proxy(socket)
        }
        Err(headless::OpenError::Other(why)) => Err(why),
    }
}

/// Whether agents are given `save`: as the settings say; where they say
/// nothing, only where no window shows the document (`window` false),
/// for there nobody else can save it.
fn agents_save(ai: &config::Ai, window: bool) -> bool {
    ai.save.unwrap_or(!window)
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

/// Where zetwerk keeps what has to outlive a session: `$XDG_STATE_HOME/zetwerk`,
/// or `~/.local/state/zetwerk`.
fn state_dir() -> Result<PathBuf, String> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let base = match (set("XDG_STATE_HOME"), set("HOME")) {
        (Some(state), _) => PathBuf::from(state),
        (None, Some(home)) => PathBuf::from(home).join(".local/state"),
        (None, None) => {
            return Err(
                "neither XDG_STATE_HOME nor HOME is set: nowhere to keep journals".to_owned(),
            );
        }
    };
    Ok(base.join("zetwerk"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    /// What the command line `line` (without the program's name) says.
    fn parsed(line: &[&str]) -> Args {
        parse(lexopt::Parser::from_args(line.iter().copied()))
            .unwrap_or_else(|why| panic!("{why}"))
            .unwrap_or_else(|| panic!("help was asked for"))
    }

    #[test]
    fn agents_save_where_the_settings_say_and_else_only_without_a_window() {
        let ai = |save| config::Ai {
            save,
            ..config::Ai::default()
        };
        assert!(!agents_save(&ai(None), true));
        assert!(agents_save(&ai(None), false));
        assert!(
            !agents_save(&ai(Some(false)), false),
            "save = false holds headless too"
        );
        assert!(agents_save(&ai(Some(true)), true));
    }

    #[test]
    fn without_a_file_the_current_directory_is_opened() {
        let args = parsed(&[]);
        assert!(args.command == Command::Open);
        assert_eq!(input(&args), Ok(PathBuf::from(".")));
        // A file is opened as it always was.
        let args = parsed(&["paper.typ", "--power", "saver"]);
        assert_eq!(input(&args), Ok(PathBuf::from("paper.typ")));
        assert_eq!(args.power, Some(PowerMode::Saver));
        assert!(input(&parsed(&["a.typ", "b.typ"])).is_err());
        // Rendering needs a file all the same.
        let args = parsed(&["render"]);
        assert!(args.command == Command::Render);
        assert!(input(&args).is_err_and(|why| why.starts_with("missing input file")));
        assert!(parse(lexopt::Parser::from_args(["--help"])).is_ok_and(|args| args.is_none()));
    }

    #[test]
    fn a_directory_is_opened_as_it_is_and_a_new_file_is_begun() {
        let directory = std::env::temp_dir().join(format!("zetwerk-main-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        assert_eq!(begin_if_new(&directory), Ok(()));
        assert_eq!(main_of(&directory, None), None);
        let new = directory.join("new.typ");
        assert_eq!(begin_if_new(&new), Ok(()));
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "");
        // What is there is never begun anew.
        std::fs::write(&new, "Text.\n").unwrap();
        assert_eq!(begin_if_new(&new), Ok(()));
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "Text.\n");
        let _ = std::fs::remove_dir_all(directory);
    }
}
