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
use crate::error::{CliError, ErrorKind, Reason, usage};
use crate::extract::TextReport;
use crate::output::{Manifest, base64_len, format_from_extension};
use crate::probe::ProbeReport;

/// Newest first; a client asking for another version gets the newest.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// Sheets per call unless `max_sheets` is given: each sheet is a full image in the context.
const DEFAULT_MAX_SHEETS: usize = 4;
/// Images returned by one call, as base64.
const MAX_RESULT_BASE64_BYTES: usize = 20_000_000;

const INSTRUCTIONS: &str = "gridity arranges PDF pages, video frames, GIF/APNG frames and \
images into labeled grid sheets sized for Claude's vision limits. Typical flow: probe the \
input, read PDF text pages with `text`, then `render` only what needs to be seen. Call \
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
    count: Option<usize>,
    /// Frames at explicit times: "0:10,1:30".
    at: Option<String>,
    /// Region of every page/frame/image as X,Y,W,H fractions, e.g. "0,0.5,1,0.5".
    crop: Option<String>,
    /// Sheet size limits (default compat).
    preset: Option<Preset>,
    /// Cell size: low = more items per sheet, high = larger cells (default medium).
    detail: Option<Detail>,
    /// Items per sheet (overrides detail).
    per_sheet: Option<usize>,
    cols: Option<u32>,
    rows: Option<u32>,
    /// Fail instead of writing more sheets than this (default 4 here).
    max_sheets: Option<usize>,
    /// Image format (default png for PDF/images, jpeg for video/animations).
    format: Option<Format>,
    /// Label template, e.g. "p.{page}"; variables depend on the input type.
    label: Option<String>,
    /// Header text instead of the file name.
    title: Option<String>,
    no_header: Option<bool>,
    no_label: Option<bool>,
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
    #[serde(rename = "type")]
    kind: Option<InputKind>,
    preset: Option<Preset>,
    /// Analyze every PDF page: content type, text size, images and drawings.
    per_page: Option<bool>,
    password_file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TextParams {
    /// PDF file.
    input: String,
    /// Pages, 1-based: "1-3,5,10-".
    pages: Option<String>,
    password_file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GuideParams {
    /// index (default), recipes, pdf, text, video, images, layout, output, mcp or all.
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
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_line(&line) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

struct Server {
    roots: Vec<PathBuf>,
}

impl Server {
    /// The response to one message, or None for notifications.
    fn handle_line(&self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(e) => return Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
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
        if !matches!(name, "render" | "probe" | "text" | "guide") {
            return Err((-32602, format!("unknown tool: {name}")));
        }
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| match name {
            "render" => self.render(parse_params(arguments)?),
            "probe" => self.probe(parse_params(arguments)?),
            "text" => self.text(parse_params(arguments)?),
            _ => guide(parse_params(arguments)?),
        }))
        .unwrap_or_else(|panic| {
            Err(CliError::new(
                ErrorKind::Runtime,
                Reason::Internal,
                format!("internal error: {}", crate::panic_message(&*panic)),
            )
            .into())
        });
        Ok(outcome.unwrap_or_else(|err| tool_error(&crate::describe(&err))))
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
        argv.value("password-file", self.password_file(p.password_file)?);
        let Command::Render(args) = argv.parse(self.inputs(p.inputs)?)? else {
            unreachable!("argv starts with render")
        };
        let manifest = crate::render::execute(*args)?.manifest;
        let images = if p.images.unwrap_or(true) {
            sheet_images(&manifest)?
        } else {
            Vec::new()
        };
        let summary = format!(
            "{} sheet(s); cell rects are [x, y, width, height] in sheet pixels.",
            manifest.sheets.len()
        );
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
        Ok(success(&report, "probe report:", Vec::new()))
    }

    fn text(&self, p: TextParams) -> Result<Value> {
        let mut argv = Argv::new("text");
        argv.value("pages", p.pages);
        argv.value("password-file", self.password_file(p.password_file)?);
        let Command::Text(args) = argv.parse(self.inputs(vec![p.input])?)? else {
            unreachable!("argv starts with text")
        };
        let report: TextReport = crate::extract::report(args)?;
        let visual = report.visual_pages();
        let summary = if visual.is_empty() {
            "page text:".to_string()
        } else {
            format!(
                "page text; p.{} show formulas, figures or text that extraction cannot capture, render them:",
                crate::render::compact_ranges(&visual)
            )
        };
        Ok(success(&report, &summary, Vec::new()))
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
                usage(Reason::InvalidArgument, message.trim_start_matches("error: "))
                    .with_hint("parameters are the CLI flags with underscores: per_sheet for --per-sheet; see guide topic \"mcp\"")
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

/// Every sheet as an image block, unless together they are too large for one result.
fn sheet_images(manifest: &Manifest) -> Result<Vec<Value>> {
    let total: usize = manifest
        .sheets
        .iter()
        .map(|s| base64_len(s.bytes.unwrap_or(0)))
        .sum();
    if total > MAX_RESULT_BASE64_BYTES {
        return Err(usage(
            Reason::TooLarge,
            format!(
                "the sheets are {:.1} MB as base64, more than the {} MB one result may carry",
                total as f64 / 1e6,
                MAX_RESULT_BASE64_BYTES / 1_000_000
            ),
        )
        .with_hint("render fewer pages or frames, or pass images: false and read the files")
        .into());
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let mut images = Vec::new();
    for sheet in &manifest.sheets {
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

/// A successful tool result: a summary line with the JSON document as text (clients that drop
/// `structuredContent` still show it to the model), then the images.
fn success(document: &impl serde::Serialize, summary: &str, images: Vec<Value>) -> Value {
    let structured = serde_json::to_value(document).unwrap_or_default();
    let mut content = vec![json!({
        "type": "text",
        "text": format!("{summary}\n{structured}"),
    })];
    content.extend(images);
    json!({ "content": content, "structuredContent": structured, "isError": false })
}

/// A failed tool call. No `structuredContent`, which would have to match the output schema.
fn tool_error(error: &CliError) -> Value {
    let mut text = format!("error: {}\nreason: {}", error.message, error.reason.name());
    if let Some(hint) = &error.hint {
        text.push_str(&format!("\nhint: {hint}"));
    }
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
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
        tool(
            "probe",
            "Page count, duration, resolution and sheet estimates for an input; with per_page, \
             which PDF pages need to be seen as images. Returns suggested render commands.",
            schema_for!(ProbeParams),
            Some(schema_for!(ProbeReport)),
            read_only.clone(),
        ),
        tool(
            "text",
            "Visible text of PDF pages with each page's content type. Reading text costs far \
             fewer tokens than images; render pages marked mixed or graphics.",
            schema_for!(TextParams),
            Some(schema_for!(TextReport)),
            read_only.clone(),
        ),
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
        assert_eq!(names, ["render", "probe", "text", "guide"]);
        assert_eq!(tools[0]["inputSchema"]["required"], json!(["inputs"]));
        assert_eq!(tools[0]["outputSchema"]["type"], "object");
        assert!(tools[0]["inputSchema"]["properties"]["password"].is_null());
    }
}
