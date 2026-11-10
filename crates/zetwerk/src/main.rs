//! zetwerk command line.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use lexopt::prelude::*;
use zetwerk_typst::{Environment, FontSet, Project};

const USAGE: &str = "\
Usage: zetwerk render <file.typ> [options]

Compiles a Typst document and writes one page as a PNG.

Options:
  -o, --output <path>   Where to write the PNG (default: <file>.png)
  -p, --page <n>        Page to render, starting at 1 (default: 1)
  -s, --scale <n>       Pixels per point (default: 2)
      --embedded-fonts  Use only the fonts shipped with Typst
  -h, --help            Show this help";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut parser = lexopt::Parser::from_env();
    match parser.next().map_err(|err| err.to_string())? {
        Some(Value(command)) if command == "render" => render(&mut parser),
        Some(Short('h') | Long("help")) => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

struct RenderArgs {
    input: PathBuf,
    output: PathBuf,
    page: usize,
    scale: f64,
    font_set: FontSet,
}

fn parse_render_args(parser: &mut lexopt::Parser) -> Result<Option<RenderArgs>, String> {
    let fail = |err: lexopt::Error| err.to_string();
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut page = 1_usize;
    let mut scale = 2.0_f64;
    let mut font_set = FontSet::EmbeddedAndSystem;
    while let Some(arg) = parser.next().map_err(fail)? {
        match arg {
            Short('o') | Long("output") => {
                output = Some(parser.value().map_err(fail)?.into());
            }
            Short('p') | Long("page") => {
                page = parser.value().and_then(|v| v.parse()).map_err(fail)?;
            }
            Short('s') | Long("scale") => {
                scale = parser.value().and_then(|v| v.parse()).map_err(fail)?;
            }
            Long("embedded-fonts") => font_set = FontSet::Embedded,
            Short('h') | Long("help") => return Ok(None),
            Value(path) if input.is_none() => input = Some(path.into()),
            other => return Err(fail(other.unexpected())),
        }
    }
    let input = input.ok_or("missing input file\n\n".to_owned() + USAGE)?;
    if page == 0 {
        return Err("pages are numbered from 1".to_owned());
    }
    if !(scale.is_finite() && scale > 0.0 && scale <= 16.0) {
        return Err("scale must be between 0 and 16".to_owned());
    }
    let output = output.unwrap_or_else(|| {
        let mut name = OsString::from(input.as_os_str());
        name.push(".png");
        name.into()
    });
    Ok(Some(RenderArgs {
        input,
        output,
        page,
        scale,
        font_set,
    }))
}

fn render(parser: &mut lexopt::Parser) -> Result<(), String> {
    let Some(args) = parse_render_args(parser)? else {
        println!("{USAGE}");
        return Ok(());
    };
    let project = Project::open(Environment::new(args.font_set), &args.input)
        .map_err(|err| err.to_string())?;
    let world = project.snapshot();
    let compiled = zetwerk_typst::compile(&world);
    for warning in &compiled.warnings {
        eprintln!("{}", zetwerk_typst::describe(&world, warning));
    }
    if compiled.panicked {
        return Err("the Typst compiler crashed on this document".to_owned());
    }
    let Some(document) = compiled.document else {
        let errors: Vec<String> = compiled
            .errors
            .iter()
            .map(|error| zetwerk_typst::describe(&world, error))
            .collect();
        return Err(errors.join("\n"));
    };
    let pages = document.pages();
    let page = pages.get(args.page - 1).ok_or_else(|| {
        format!(
            "page {} does not exist; the document has {} page(s)",
            args.page,
            pages.len()
        )
    })?;
    zetwerk_render::render_page(page, args.scale)
        .save_png(&args.output)
        .map_err(|err| format!("cannot write {}: {err}", args.output.display()))?;
    println!(
        "{} (page {} of {})",
        args.output.display(),
        args.page,
        pages.len()
    );
    Ok(())
}
