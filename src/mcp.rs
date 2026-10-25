//! `gridity mcp`: a Model Context Protocol server on stdin/stdout. Each line is one JSON-RPC
//! message. Tool calls are turned into command-line arguments and parsed by clap, so they
//! share every check and default with the CLI.

use std::fmt::Display;
use std::io::{BufRead, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use anyhow::Result;
use base64::Engine;
use clap::{Parser, ValueEnum};
use schemars::{JsonSchema, Schema, schema_for};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::cli::{Cli, Command, Detail, Format, InputKind, McpArgs, Preset};
use crate::error::{CliError, Reason, describe, usage};
use crate::layout::MAX_SHEETS;
use crate::output::{Manifest, base64_len, format_from_extension};
use crate::probe::ProbeReport;

/// Newest first; a client asking for another version gets the newest.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// Sheets per call unless `max_sheets` is given: each sheet is a full image in the context.
pub const DEFAULT_MAX_SHEETS: usize = 4;
/// Images returned by one call, as base64.
const MAX_RESULT_BASE64_BYTES: usize = 20_000_000;

const INSTRUCTIONS: &str = "gridity arranges PDF pages, video frames, GIF/APNG frames and \
images into labeled grid sheets sized for Claude's vision limits. Typical flow: probe the \
input (its suggested_calls are ready-made tool calls; with per_page it lists the PDF pages \
that a text extractor reads for fewer tokens), then `render` only what needs to be seen. To \
zoom in, render with `ruler: true` and pass the values read off the ruler as `crop`. Call \
`guide` for options and recipes.";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenderParams {
    /// Input file, several image files, or one directory of images.
    #[schemars(length(min = 1))]
    inputs: Vec<String>,
    /// Input type, detected when omitted.
    #[serde(rename = "type")]
    kind: Option<InputKind>,
    /// PDF pages, 1-based: "1-3,5,10-", "-4".
    pages: Option<String>,
    /// Video/animation start time: "90", "1:30", "1m30s".
    start: Option<String>,
    /// Video/animation end time.
    end: Option<String>,
    /// One frame every interval, e.g. "5s".
    every: Option<String>,
    /// Number of evenly spaced frames.
    #[schemars(range(min = 1))]
    count: Option<usize>,
    /// Frames at explicit times: "0:10,1:30".
    at: Option<String>,
    /// Region of every page/frame/image as X,Y,W,H fractions, e.g. "0,0.5,1,0.5". Video,
    /// GIF and image regions keep their native pixels and are never enlarged.
    crop: Option<String>,
    /// Sheet size limits (default compat).
    preset: Option<Preset>,
    /// Cell size: low = more items per sheet, high = larger cells (default medium).
    detail: Option<Detail>,
    /// Items per sheet (overrides detail).
    #[schemars(range(min = 1, max = 10_000))]
    per_sheet: Option<usize>,
    /// Columns per sheet.
    #[schemars(range(min = 1, max = 100))]
    cols: Option<u32>,
    /// Rows per sheet.
    #[schemars(range(min = 1, max = 100))]
    rows: Option<u32>,
    /// Fail instead of writing more sheets than this (default 4 here).
    #[schemars(range(min = 1, max = MAX_SHEETS))]
    max_sheets: Option<usize>,
    /// Image format (default png for PDF/images, jpeg for video/animations).
    format: Option<Format>,
    /// Label template, e.g. "p.{page}"; variables depend on the input type.
    label: Option<String>,
    /// Header text instead of the file name.
    title: Option<String>,
    /// Do not draw the header.
    no_header: Option<bool>,
    /// Do not draw labels.
    no_label: Option<bool>,
    /// Draw a ruler over every cell, valued in fractions of the whole page or frame (the
    /// coordinates crop takes, also inside a cropped cell).
    ruler: Option<bool>,
    /// Ruler step, a multiple of 0.01 up to 0.5 (implies ruler; default automatic).
    #[schemars(range(min = 0.01, max = 0.5))]
    ruler_step: Option<f64>,
    /// File holding the PDF password (or set GRIDITY_PDF_PASSWORD for the server).
    password_file: Option<String>,
    /// Return the sheets as images (default true); false returns paths and the manifest only.
    images: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProbeParams {
    /// Input file, several image files, or one directory of images.
    #[schemars(length(min = 1))]
    inputs: Vec<String>,
    /// Input type, detected when omitted.
    #[serde(rename = "type")]
    kind: Option<InputKind>,
    /// Size limits used for the sheet estimates (default compat).
    preset: Option<Preset>,
    /// Analyze every PDF page: content type, text size, images and drawings.
    per_page: Option<bool>,
    /// File holding the PDF password (or set GRIDITY_PDF_PASSWORD for the server).
    password_file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GuideParams {
    /// Guide topic (default index), or all for every topic.
    #[schemars(extend("type" = "string", "enum" = crate::guide::topic_names()))]
    topic: Option<String>,
}

pub fn run(args: McpArgs) -> Result<()> {
    let server = Server {
        roots: args
            .root
            .iter()
            .map(|root| {
                std::fs::canonicalize(root).map_err(|e| {
                    usage(
                        Reason::InvalidValue,
                        format!("--root {}: {e}", root.display()),
                    )
                })
            })
            .collect::<Result<_, _>>()?,
    };
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout();
    let mut line = Vec::new();
    loop {
        line.clear();
        if stdin.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let message = line.trim_ascii();
        if message.is_empty() {
            continue;
        }
        if let Some(response) = server.handle_bytes(message) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
}

struct Server {
    roots: Vec<PathBuf>,
}

impl Server {
    /// A line that is not UTF-8 is answered as a parse error, like any other invalid JSON.
    fn handle_bytes(&self, line: &[u8]) -> Option<Value> {
        match std::str::from_utf8(line) {
            Ok(line) => self.handle_line(line),
            Err(e) => Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        }
    }

    /// The response to one line: a message, or a JSON-RPC batch answered with an array.
    /// None when the line holds only notifications.
    fn handle_line(&self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(e) => return Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        match message {
            Value::Array(batch) if batch.is_empty() => Some(rpc_error(
                Value::Null,
                -32600,
                "invalid request: empty batch",
            )),
            Value::Array(batch) => {
                let responses: Vec<Value> = batch
                    .iter()
                    .filter_map(|message| self.handle_message(message))
                    .collect();
                if responses.is_empty() {
                    None
                } else {
                    Some(Value::Array(responses))
                }
            }
            message => self.handle_message(&message),
        }
    }

    /// The response to one message, or None for notifications.
    fn handle_message(&self, message: &Value) -> Option<Value> {
        if !message.is_object() {
            return Some(rpc_error(Value::Null, -32600, "invalid request"));
        }
        // Notifications carry no id and get no response.
        let id = message.get("id").cloned()?;
        let Some(method) = message["method"].as_str() else {
            return Some(rpc_error(id, -32600, "invalid request"));
        };
        let params = &message["params"];
        let result = match method {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => self.call(params),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => rpc_error(id, code, &message),
        })
    }

    /// Runs a tool. Protocol errors are only for unknown tools and malformed requests; every
    /// failure of the tool itself is a result with `isError`, so the model can correct it.
    fn call(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params["name"].as_str().unwrap_or_default();
        let arguments = match &params["arguments"] {
            Value::Null => json!({}),
            Value::Object(_) => params["arguments"].clone(),
            _ => return Err((-32602, "arguments must be an object".to_string())),
        };
        if !matches!(name, "render" | "probe" | "guide") {
            return Err((-32602, format!("unknown tool: {name}")));
        }
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| match name {
            "render" => self.render(parse_params(arguments)?),
            "probe" => self.probe(parse_params(arguments)?),
            _ => guide(parse_params(arguments)?),
        }))
        .unwrap_or_else(|panic| Err(crate::error::from_panic(&*panic).into()));
        Ok(outcome.unwrap_or_else(|err| tool_error(&describe(&err))))
    }

    fn render(&self, p: RenderParams) -> Result<Value> {
        let mut argv = Argv::new("render");
        argv.value("type", p.kind.map(value_name));
        argv.value("pages", p.pages);
        argv.value("start", p.start);
        argv.value("end", p.end);
        argv.value("every", p.every);
        argv.value("count", p.count);
        argv.value("at", p.at);
        argv.value("crop", p.crop);
        argv.value("preset", p.preset.map(value_name));
        argv.value("detail", p.detail.map(value_name));
        argv.value("per-sheet", p.per_sheet);
        argv.value("cols", p.cols);
        argv.value("rows", p.rows);
        argv.value(
            "max-sheets",
            Some(p.max_sheets.unwrap_or(DEFAULT_MAX_SHEETS)),
        );
        argv.value("format", p.format.map(value_name));
        argv.value("label", p.label);
        argv.value("title", p.title);
        argv.switch("no-header", p.no_header);
        argv.switch("no-label", p.no_label);
        argv.switch("ruler", p.ruler);
        argv.value("ruler-step", p.ruler_step);
        argv.value("password-file", self.password_file(p.password_file)?);
        let Command::Render(args) = argv.parse(self.inputs(p.inputs)?)? else {
            unreachable!("argv starts with render")
        };
        let manifest = crate::render::execute(*args)?.manifest;
        let total = manifest.sheets.len();
        let (images, attached) = if p.images.unwrap_or(true) {
            let images = sheet_images(&manifest)?;
            let attached = images.len();
            (images, attached)
        } else {
            (Vec::new(), total)
        };
        let mut summary = format!("{total} sheet(s); ");
        if attached < total {
            summary.push_str(&partial_summary(attached, total));
            summary.push(' ');
        }
        summary.push_str("cell rects are [x, y, width, height] in sheet pixels.");
        Ok(success(&manifest, &summary, images))
    }

    fn probe(&self, p: ProbeParams) -> Result<Value> {
        let mut argv = Argv::new("probe");
        argv.value("type", p.kind.map(value_name));
        argv.value("preset", p.preset.map(value_name));
        argv.switch("per-page", p.per_page);
        argv.value("password-file", self.password_file(p.password_file)?);
        let Command::Probe(args) = argv.parse(self.inputs(p.inputs)?)? else {
            unreachable!("argv starts with probe")
        };
        let report: ProbeReport = crate::probe::report(args)?;
        Ok(success(
            &report,
            "probe report; make each suggested_calls entry as given (tool + arguments):",
            Vec::new(),
        ))
    }

    /// Input paths, checked against `--root` when one is given.
    fn inputs(&self, inputs: Vec<String>) -> Result<Vec<String>, CliError> {
        for input in &inputs {
            self.check_root(Path::new(input))?;
        }
        Ok(inputs)
    }

    fn password_file(&self, path: Option<String>) -> Result<Option<String>, CliError> {
        let Some(path) = path else {
            return Ok(None);
        };
        if path == "-" {
            return Err(usage(
                Reason::InvalidValue,
                "password_file cannot be \"-\": stdin carries the MCP messages",
            )
            .with_hint("give a file path, or start the server with GRIDITY_PDF_PASSWORD set"));
        }
        self.check_root(Path::new(&path))?;
        Ok(Some(path))
    }

    /// Paths that do not exist pass; gridity then reports them as not found. A directory's
    /// entries are checked too, since a link inside it may point anywhere.
    fn check_root(&self, path: &Path) -> Result<(), CliError> {
        if self.roots.is_empty() {
            return Ok(());
        }
        let Ok(path) = std::fs::canonicalize(path) else {
            return Ok(());
        };
        if !self.inside_roots(&path) {
            return Err(self.outside_root(&path));
        }
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                if let Ok(target) = std::fs::canonicalize(entry.path())
                    && !self.inside_roots(&target)
                {
                    return Err(self.outside_root(&entry.path()));
                }
            }
        }
        Ok(())
    }

    fn inside_roots(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    fn outside_root(&self, path: &Path) -> CliError {
        usage(
            Reason::OutsideRoot,
            format!(
                "{} is outside the directories this server may read",
                path.display()
            ),
        )
        .with_hint(format!(
            "allowed: {}",
            self.roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Command-line arguments built from tool parameters. Values are passed as one `--name=value`
/// argument and inputs follow `--`, so a value starting with `-` is never read as a flag.
struct Argv(Vec<String>);

impl Argv {
    fn new(command: &str) -> Argv {
        Argv(vec!["gridity".to_string(), command.to_string()])
    }

    fn value(&mut self, name: &str, value: Option<impl Display>) {
        if let Some(value) = value {
            self.0.push(format!("--{name}={value}"));
        }
    }

    fn switch(&mut self, name: &str, on: Option<bool>) {
        if on == Some(true) {
            self.0.push(format!("--{name}"));
        }
    }

    fn parse(mut self, inputs: Vec<String>) -> Result<Command, CliError> {
        self.0.push("--".to_string());
        self.0.extend(inputs);
        Cli::try_parse_from(&self.0)
            .map(|cli| cli.command)
            .map_err(|e| {
                let rendered = e.to_string();
                let message = rendered.lines().next().unwrap_or_default();
                usage(
                    Reason::InvalidArgument,
                    message.trim_start_matches("error: "),
                )
                .with_hint("call tools/list for the parameter schema; see guide topic \"mcp\"")
            })
    }
}

fn value_name(value: impl ValueEnum) -> String {
    value
        .to_possible_value()
        .map(|v| v.get_name().to_string())
        .unwrap_or_default()
}

fn parse_params<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments).map_err(|e| {
        usage(Reason::InvalidArgument, format!("invalid arguments: {e}"))
            .with_hint("call tools/list for the parameter schema")
            .into()
    })
}

fn guide(p: GuideParams) -> Result<Value> {
    let text = crate::guide::text(p.topic.as_deref())?;
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
}

/// The first sheets as image blocks, as many as fit in one result; the rest stay files.
fn sheet_images(manifest: &Manifest) -> Result<Vec<Value>> {
    let sizes: Vec<usize> = manifest
        .sheets
        .iter()
        .map(|s| base64_len(s.bytes.unwrap_or(0)))
        .collect();
    let count = attachable(&sizes, MAX_RESULT_BASE64_BYTES);
    let engine = base64::engine::general_purpose::STANDARD;
    let mut images = Vec::new();
    for sheet in &manifest.sheets[..count] {
        let Some(path) = &sheet.path else { continue };
        let mime = match format_from_extension(Path::new(path)) {
            Some(Format::Jpeg) => "image/jpeg",
            _ => "image/png",
        };
        let data = std::fs::read(path)?;
        images.push(json!({ "type": "image", "data": engine.encode(data), "mimeType": mime }));
    }
    Ok(images)
}

/// How many sheets, from the first, fit together within `budget` base64 bytes.
fn attachable(base64_sizes: &[usize], budget: usize) -> usize {
    let mut used = 0;
    base64_sizes
        .iter()
        .take_while(|&&size| {
            used += size;
            used <= budget
        })
        .count()
}

/// Which sheets a result leaves out because of `MAX_RESULT_BASE64_BYTES`, and how to see them.
fn partial_summary(attached: usize, total: usize) -> String {
    let sheets = |first: usize, last: usize| {
        if first == last {
            format!("sheet {first}")
        } else {
            format!("sheets {first}-{last}")
        }
    };
    let limit = format!(
        "one result carries at most {} MB of images (base64)",
        MAX_RESULT_BASE64_BYTES / 1_000_000
    );
    let missing = sheets(attached + 1, total);
    if attached == 0 {
        return format!(
            "no sheet is attached: sheet 1 alone is larger than {limit}. The paths of \
             {missing} are in the manifest at sheets[i].path: read those files, or render \
             smaller sheets with format jpeg or preset compat."
        );
    }
    format!(
        "{} attached as images; {missing} not, since {limit}. Their paths are in the \
         manifest at sheets[i].path: read those files, or render the rest in another call \
         with pages (or start/end) narrowed to their cells.",
        sheets(1, attached)
    )
}

/// A successful tool result: a summary line with the JSON document as text (clients that drop
/// `structuredContent` still show it to the model), then the images. The text is serialized
/// from the struct, since a `Value` sorts the keys and `schema_version` would not come first.
fn success(document: &impl serde::Serialize, summary: &str, images: Vec<Value>) -> Value {
    let text = serde_json::to_string(document).unwrap_or_default();
    let structured = serde_json::to_value(document).unwrap_or_default();
    let mut content = vec![json!({
        "type": "text",
        "text": format!("{summary}\n{text}"),
    })];
    content.extend(images);
    json!({ "content": content, "structuredContent": structured, "isError": false })
}

/// A failed tool call. No `structuredContent`, which would have to match the output schema.
fn tool_error(error: &CliError) -> Value {
    let parameters = parameter_names();
    let mut text = format!(
        "error: {}\nreason: {}",
        with_parameter_names(&error.message, &parameters),
        error.reason.name()
    );
    if let Some(hint) = &error.hint {
        let hint = with_parameter_names(hint, &parameters);
        text.push_str(&format!("\nhint: {hint}"));
    }
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

fn parameter_names() -> Vec<String> {
    [
        schema_for!(RenderParams),
        schema_for!(ProbeParams),
        schema_for!(GuideParams),
    ]
    .iter()
    .filter_map(|schema| schema.get("properties")?.as_object())
    .flat_map(|properties| properties.keys().cloned())
    .collect()
}

/// Errors name CLI flags; a tool caller gets `max_sheets 6` for `--max-sheets 6`, `'cols'` for
/// clap's `'--cols <N>'`, and `--gap (CLI only)` for flags without a parameter. Only whole
/// tokens are flags, so paths such as `/tmp/--pages/a.pdf` stay as they are.
fn with_parameter_names(text: &str, parameters: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut last_flag_end = None;
    while let Some(found) = text[copied..].find("--") {
        let start = copied + found;
        let name_end = text[start + 2..]
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            .map_or(text.len(), |n| start + 2 + n);
        let name = &text[start + 2..name_end];
        out.push_str(&text[copied..start]);
        copied = name_end;
        let starts_flag = match text[..start].chars().next_back() {
            None => true,
            Some('/') => last_flag_end == Some(start - 1),
            Some(c) => c.is_whitespace() || "'\"`([".contains(c),
        };
        let ends_flag = {
            let rest = &text[name_end..];
            match rest.chars().next() {
                None => true,
                Some('.') => rest[1..].chars().next().is_none_or(char::is_whitespace),
                Some('/') => rest[1..].starts_with("--"),
                Some(c) => !(c.is_alphanumeric() || c == '_' || c == '\\'),
            }
        };
        if !(starts_flag && ends_flag && name.starts_with(|c: char| c.is_ascii_lowercase())) {
            out.push_str(&text[start..name_end]);
            continue;
        }
        last_flag_end = Some(name_end);
        let parameter = name.replace('-', "_");
        if parameters.contains(&parameter) {
            out.push_str(&parameter);
            copied += value_placeholder_len(&text[name_end..]);
        } else {
            out.push_str(&text[start..name_end]);
            out.push_str(" (CLI only)");
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Length of a ` <N>` or ` <X,Y,W,H>` value name after a flag.
fn value_placeholder_len(rest: &str) -> usize {
    let Some(inner) = rest.strip_prefix(" <") else {
        return 0;
    };
    match inner.find('>') {
        Some(n)
            if n > 0
                && inner[..n]
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == ',') =>
        {
            n + 3
        }
        _ => 0,
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let requested = params["protocolVersion"].as_str().unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == requested)
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "gridity", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

fn tools() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    vec![
        tool(
            "render",
            "Render grid sheets from a PDF, a video, a GIF/APNG or images and return them as \
             images, with a JSON manifest (cell rects, labels, pages or times). Sheets are also \
             written to a private temporary directory. At most 4 sheets unless max_sheets is set.",
            schema_for!(RenderParams),
            Some(schema_for!(Manifest)),
            json!({ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }),
        ),
        large_text(tool(
            "probe",
            "Page count, duration, resolution and sheet estimates for an input; with per_page, \
             which PDF pages need to be seen as images and which a text extractor reads \
             (text_pages). Returns suggested_calls: tool calls to \
             make as given.",
            schema_for!(ProbeParams),
            Some(schema_for!(ProbeReport)),
            read_only.clone(),
        )),
        tool(
            "guide",
            "gridity documentation in Markdown: options, recipes, output schema.",
            schema_for!(GuideParams),
            None,
            read_only,
        ),
    ]
}

fn tool(
    name: &str,
    description: &str,
    input: Schema,
    output: Option<Schema>,
    annotations: Value,
) -> Value {
    let mut tool = json!({
        "name": name,
        "description": description,
        "inputSchema": input,
        "annotations": annotations,
    });
    if let Some(output) = output {
        tool["outputSchema"] = output.into();
    }
    tool
}

/// Claude Code saves a text result over 50,000 characters to a file unless the tool declares
/// a larger limit, up to this one.
/// https://code.claude.com/docs/en/env-vars (MAX_MCP_OUTPUT_TOKENS), checked 2026-10-22.
const MAX_INLINE_TEXT_CHARS: usize = 500_000;

/// Keeps long `probe` results (`per_page` on long PDFs) in the conversation in Claude Code.
fn large_text(mut tool: Value) -> Value {
    tool["_meta"] = json!({ "anthropic/maxResultSizeChars": MAX_INLINE_TEXT_CHARS });
    tool
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server { roots: Vec::new() }
    }

    #[test]
    fn values_never_become_flags() {
        let mut argv = Argv::new("render");
        argv.value("pages", Some("-4"));
        argv.switch("no-header", Some(true));
        argv.switch("no-label", Some(false));
        assert!(argv.0.contains(&"--pages=-4".to_string()));
        let Command::Render(args) = argv.parse(vec!["-clip.pdf".to_string()]).unwrap() else {
            panic!("render")
        };
        assert_eq!(args.selection.pages.as_deref(), Some("-4"));
        assert_eq!(args.inputs, [PathBuf::from("-clip.pdf")]);
        assert!(args.style.no_header && !args.style.no_label);
    }

    #[test]
    fn negotiates_the_protocol_version() {
        let version =
            |v: &str| initialize(&json!({ "protocolVersion": v }))["protocolVersion"].clone();
        assert_eq!(version("2025-06-18"), "2025-06-18");
        assert_eq!(version("2099-01-01"), PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn protocol_errors() {
        let s = server();
        assert_eq!(s.handle_line("{nope").unwrap()["error"]["code"], -32700);
        assert_eq!(
            s.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#)
                .unwrap()["error"]["code"],
            -32601
        );
        assert!(
            s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        let call = |params: &str| {
            s.handle_line(&format!(
                r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{params}}}"#
            ))
            .unwrap()
        };
        assert_eq!(call(r#"{"name":"nope"}"#)["error"]["code"], -32602);
        let invalid = call(r#"{"name":"render","arguments":{"inputs":["x.pdf"],"bogus":1}}"#);
        assert_eq!(invalid["result"]["isError"], true);
        let missing = call(r#"{"name":"probe","arguments":{"inputs":["/nonexistent/x.pdf"]}}"#);
        assert_eq!(missing["result"]["isError"], true);
        assert!(
            missing["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("reason: not_found")
        );
    }

    #[test]
    fn roots_limit_inputs() {
        let dir = tempfile::TempDir::new().unwrap();
        let inside = dir.path().join("in.pdf");
        std::fs::write(&inside, b"%PDF-1.4").unwrap();
        let server = Server {
            roots: vec![std::fs::canonicalize(dir.path().join(".")).unwrap()],
        };
        assert!(server.check_root(&inside).is_ok());
        let outside = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
        assert_eq!(
            server
                .check_root(&outside.join("Cargo.toml"))
                .unwrap_err()
                .reason,
            Reason::OutsideRoot
        );
        assert!(server.password_file(Some("-".to_string())).is_err());

        #[cfg(unix)]
        {
            let images = dir.path().join("images");
            std::fs::create_dir(&images).unwrap();
            std::os::unix::fs::symlink(outside.join("Cargo.toml"), images.join("a.png")).unwrap();
            assert_eq!(
                server.check_root(&images).unwrap_err().reason,
                Reason::OutsideRoot
            );
        }
    }

    #[test]
    fn tools_list_their_schemas() {
        let tools = tools();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["render", "probe", "guide"]);
        assert_eq!(tools[0]["inputSchema"]["required"], json!(["inputs"]));
        assert_eq!(tools[0]["outputSchema"]["type"], "object");
        assert!(tools[0]["inputSchema"]["properties"]["password"].is_null());
    }

    #[test]
    fn batches_answer_every_request() {
        let s = server();
        let ping = |id: i64| json!({ "jsonrpc": "2.0", "id": id, "method": "ping" });
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });

        let responses = s
            .handle_line(&json!([ping(1), notification, 42, ping(2)]).to_string())
            .unwrap();
        let responses = responses.as_array().unwrap();
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"], json!({}));
        assert_eq!(responses[1]["id"], Value::Null);
        assert_eq!(responses[1]["error"]["code"], -32600);
        assert_eq!(responses[2]["id"], 2);

        let single = s.handle_line(&json!([ping(3)]).to_string()).unwrap();
        assert_eq!(single.as_array().unwrap()[0]["id"], 3);
        assert!(s.handle_line(&json!([notification]).to_string()).is_none());

        let empty = s.handle_line("[]").unwrap();
        assert!(empty.is_object());
        assert_eq!(empty["error"]["code"], -32600);
        assert_eq!(empty["id"], Value::Null);

        assert_eq!(s.handle_line("[1,").unwrap()["error"]["code"], -32700);
        assert_eq!(s.handle_line("42").unwrap()["error"]["code"], -32600);
    }

    #[test]
    fn lines_that_are_not_utf8_are_parse_errors() {
        let error = server().handle_bytes(b"\xff\xfe").unwrap();
        assert_eq!(error["error"]["code"], -32700);
        assert_eq!(error["id"], Value::Null);
    }

    #[test]
    fn result_text_keeps_schema_version_first() {
        #[derive(serde::Serialize)]
        struct Document {
            schema_version: u32,
            alpha: u32,
        }
        let result = success(
            &Document {
                schema_version: 1,
                alpha: 2,
            },
            "summary:",
            Vec::new(),
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        let (_, document) = text.split_once('\n').unwrap();
        assert_eq!(document, r#"{"schema_version":1,"alpha":2}"#);
        assert_eq!(result["structuredContent"]["alpha"], 2);
    }

    #[test]
    fn probe_results_stay_inline() {
        let tools = tools();
        assert_eq!(tools[1]["_meta"]["anthropic/maxResultSizeChars"], 500_000);
        assert!(tools[2].get("_meta").is_none());
    }

    #[test]
    fn attaches_the_sheets_that_fit() {
        assert_eq!(attachable(&[], 10), 0);
        assert_eq!(attachable(&[3, 3, 3], 10), 3);
        assert_eq!(attachable(&[4, 6, 1], 10), 2);
        assert_eq!(attachable(&[4, 7, 1], 10), 1);
        assert_eq!(attachable(&[11, 1], 10), 0);
    }

    #[test]
    fn summary_names_the_sheets_left_out() {
        let partial = partial_summary(13, 64);
        assert!(partial.contains("sheets 1-13 attached"), "{partial}");
        assert!(partial.contains("sheets 14-64 not"), "{partial}");
        assert!(partial.contains("sheets[i].path"), "{partial}");
        let last = partial_summary(1, 2);
        assert!(
            last.contains("sheet 1 attached") && last.contains("sheet 2 not"),
            "{last}"
        );
        let none = partial_summary(0, 2);
        assert!(none.contains("no sheet is attached"), "{none}");
        assert!(none.contains("sheets 1-2 are in the manifest"), "{none}");
    }

    #[test]
    fn every_parameter_is_described() {
        for tool in tools() {
            let properties = tool["inputSchema"]["properties"].as_object().unwrap();
            for (name, property) in properties {
                let description = property["description"].as_str().unwrap_or_default();
                assert!(!description.is_empty(), "{}.{name}", tool["name"]);
            }
        }
    }

    #[test]
    fn schema_ranges_match_the_cli() {
        let tools = tools();
        let render = &tools[0]["inputSchema"]["properties"];
        for (name, flag, min, max) in [
            ("cols", "cols", 1, 100),
            ("rows", "rows", 1, 100),
            ("per_sheet", "per-sheet", 1, 10_000),
            ("max_sheets", "max-sheets", 1, MAX_SHEETS as u64),
        ] {
            assert_eq!(render[name]["minimum"], min, "{name}");
            assert_eq!(render[name]["maximum"], max, "{name}");
            let parses = |value: u64| {
                let mut argv = Argv::new("render");
                argv.value(flag, Some(value));
                argv.parse(vec!["x.pdf".to_string()]).is_ok()
            };
            assert!(parses(min) && parses(max), "{name}");
            assert!(!parses(min - 1) && !parses(max + 1), "{name}");
        }
        assert_eq!(render["count"]["minimum"], 1);
        assert!(render["count"]["maximum"].is_null());
        assert!(crate::select::Sampling::from_args(None, Some(0), None).is_err());
    }

    #[test]
    fn guide_topics_are_an_enum() {
        let tools = tools();
        let schema = &tools[2]["inputSchema"];
        assert!(schema["required"].as_array().is_none_or(|r| r.is_empty()));
        let topics: Vec<&str> = schema["properties"]["topic"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(topics, crate::guide::topic_names());
        assert!(topics.contains(&"mcp") && topics.contains(&"all"));
        for topic in topics {
            assert!(crate::guide::text(Some(topic)).is_ok(), "{topic}");
        }
        let unknown = guide(GuideParams {
            topic: Some("nope".to_string()),
        });
        let error = describe(&unknown.unwrap_err());
        assert_eq!(error.reason, Reason::UnknownTopic);
    }

    #[test]
    fn errors_name_tool_parameters() {
        let parameters = parameter_names();
        let rewrite = |text: &str| with_parameter_names(text, &parameters);

        let sheets = crate::layout::check_sheets(20, 4, 4).unwrap_err();
        assert_eq!(
            rewrite(&sheets.message),
            "20 items need 5 sheets at 4 per sheet, more than max_sheets 4"
        );
        assert_eq!(
            rewrite(&sheets.hint.unwrap()),
            "narrow the selection (pages, start/end, count, every), use detail low, or pass max_sheets 5"
        );
        assert_eq!(
            rewrite("lower --per-sheet, --cols/--rows, --gap or --padding, or use --preset hires"),
            "lower per_sheet, cols/rows, --gap (CLI only) or --padding (CLI only), or use preset hires"
        );
        assert_eq!(
            rewrite("pass --password-file <FILE> or set X; avoid --password, which others see"),
            "pass password_file or set X; avoid --password (CLI only), which others see"
        );
        assert_eq!(rewrite("--type."), "type.");
        for unchanged in [
            "cannot read /tmp/--pages/a.pdf",
            "cannot read a--count.pdf",
            "cannot read --pages.pdf",
            "cannot read C:\\--every\\b.png",
            "x --/--pages -- --9",
        ] {
            assert_eq!(rewrite(unchanged), unchanged);
        }
    }

    #[test]
    fn clap_errors_name_tool_parameters() {
        let text = |argv: Argv| {
            let error = argv.parse(vec!["x.pdf".to_string()]).unwrap_err();
            tool_error(&error)["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let mut cols = Argv::new("render");
        cols.value("cols", Some(101));
        let cols = text(cols);
        assert!(
            cols.contains("for 'cols'") && !cols.contains("--"),
            "{cols}"
        );
        let mut conflict = Argv::new("render");
        conflict.value("title", Some("T"));
        conflict.switch("no-header", Some(true));
        let conflict = text(conflict);
        assert!(
            conflict.contains("'title'") && conflict.contains("'no_header'"),
            "{conflict}"
        );
    }
}
