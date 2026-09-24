//! In-app photo agent and localhost MCP server.
//!
//! The same tools answer the chat panel and external agents. Metadata edits
//! are written into the photo (JPEG IPTC/XMP, or an XMP sidecar) and into the
//! Lap catalog.

use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use crate::t_config::{
    load_app_config, save_app_config, CustomSmartAlbumState, SmartAlbumGroupState, SmartAlbumQueryState,
    SmartAlbumRuleState, SmartAlbumSortState,
};
use crate::t_sqlite::{ACollection, AFile, ATag, Album, QueryParams, SmartQueryParams};

const DEFAULT_PORT: u16 = 47321;
const MAX_TOOL_ROUNDS: usize = 6;
const MAX_BODY: usize = 1024 * 1024;

static APP: OnceLock<AppHandle> = OnceLock::new();
static SERVER: Mutex<Option<tauri::async_runtime::JoinHandle<()>>> = Mutex::new(None);
static MCP_RUNNING: AtomicBool = AtomicBool::new(false);
static MCP_ERROR: Mutex<String> = Mutex::new(String::new());

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSettings {
    provider: String,
    base_url: String,
    model: String,
    api_key: String,
    mcp_enabled: bool,
    mcp_port: u16,
    mcp_token: String,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: String::new(),
            api_key: String::new(),
            mcp_enabled: true,
            mcp_port: DEFAULT_PORT,
            mcp_token: Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettingsInput {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub mcp_enabled: bool,
    pub mcp_port: u16,
    pub regenerate_token: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentContext {
    #[serde(default)]
    pub file_ids: Vec<i64>,
    pub focused_file_id: Option<i64>,
    pub album_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatResponse {
    pub message: String,
    pub actions: Vec<Value>,
    pub tools_used: Vec<String>,
    pub trace: Vec<Value>,
}

pub fn install(app: &AppHandle) {
    let _ = APP.set(app.clone());
    restart_mcp();
}

#[tauri::command]
pub fn get_agent_settings() -> Result<Value, String> {
    let settings = load_settings()?;
    Ok(settings_view(&settings))
}

#[tauri::command]
pub fn save_agent_settings(input: AgentSettingsInput) -> Result<Value, String> {
    let mut settings = load_settings()?;
    let provider = input.provider.trim().to_ascii_lowercase();
    settings.provider = if provider.is_empty() { "custom".to_string() } else { provider };
    settings.base_url = normalize_base_url(&input.base_url);
    if settings.base_url.is_empty() {
        settings.base_url = preset_base_url(&settings.provider).to_string();
    }
    settings.model = input.model.trim().to_string();
    if let Some(api_key) = input.api_key {
        settings.api_key = api_key.trim().to_string();
    }
    settings.mcp_enabled = input.mcp_enabled;
    settings.mcp_port = input.mcp_port.clamp(1024, 65535);
    if input.regenerate_token || settings.mcp_token.trim().is_empty() {
        settings.mcp_token = Uuid::new_v4().to_string();
    }
    save_settings(&settings)?;
    restart_mcp();
    Ok(settings_view(&settings))
}

#[tauri::command]
pub async fn agent_list_models() -> Result<Vec<String>, String> {
    let settings = load_settings()?;
    if settings.base_url.is_empty() {
        return Err("Set a provider URL first".to_string());
    }
    let client = http_client()?;
    let mut request = client.get(format!("{}/models", settings.base_url.trim_end_matches('/')));
    if !settings.api_key.is_empty() {
        request = request.bearer_auth(&settings.api_key);
    }
    let response = request.send().await.map_err(|error| format!("Could not list models: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("Model list failed ({status}): {}", trim_error(&body)));
    }
    let parsed: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let openrouter = settings.provider == "openrouter" || settings.base_url.contains("openrouter.ai");
    let mut models = parsed
        .get("data")
        .and_then(|data| data.as_array())
        .map(|rows| {
            let ids = |row: &Value| row.get("id").and_then(|id| id.as_str()).map(str::to_string);
            let tool_models = rows
                .iter()
                .filter(|row| model_supports_tools(row))
                .filter_map(ids)
                .collect::<Vec<_>>();
            if openrouter && !tool_models.is_empty() {
                tool_models
            } else {
                rows.iter().filter_map(ids).collect()
            }
        })
        .unwrap_or_default();
    models.sort();
    models.dedup();
    Ok(models)
}

#[tauri::command]
pub async fn agent_chat(messages: Vec<AgentChatMessage>, context: Option<AgentContext>) -> Result<AgentChatResponse, String> {
    let settings = load_settings()?;
    if settings.model.trim().is_empty() {
        return Err("Choose a model in Settings → Agent".to_string());
    }
    if settings.provider != "ollama" && settings.api_key.trim().is_empty() {
        return Err("Add an API key in Settings → Agent".to_string());
    }
    let app = APP.get().cloned().ok_or_else(|| "Lap is still starting".to_string())?;
    let mut transcript = vec![json!({
        "role": "system",
        "content": system_prompt(&context.unwrap_or_default()),
    })];
    for message in messages.into_iter().rev().take(16).collect::<Vec<_>>().into_iter().rev() {
        if message.role != "user" && message.role != "assistant" {
            continue;
        }
        let content: String = message.content.chars().take(8000).collect();
        if content.trim().is_empty() {
            continue;
        }
        transcript.push(json!({ "role": message.role, "content": content }));
    }
    if transcript.len() < 2 {
        return Err("Write a message first".to_string());
    }

    let client = http_client()?;
    let tools = tool_definitions();
    let mut actions = Vec::new();
    let mut tools_used = Vec::new();
    let mut trace = vec![json!({
        "kind": "note",
        "text": format!("{} · {}", settings.provider, settings.model),
    })];
    let mut native_tools = true;
    for _ in 0..MAX_TOOL_ROUNDS {
        let reply = match chat_completion(&client, &settings, &transcript, native_tools.then_some(&tools)).await {
            Err(error) if native_tools && missing_tool_endpoint(&error) => {
                native_tools = false;
                trace.push(json!({
                    "kind": "note",
                    "text": "No native tool endpoint. Later calls are read from the reply text.",
                }));
                if let Some(content) = transcript.first_mut().and_then(|message| message.get_mut("content")) {
                    if let Some(text) = content.as_str() {
                        *content = json!(format!("{text}\n\n{}", text_tool_instructions()));
                    }
                }
                chat_completion(&client, &settings, &transcript, None).await?
            }
            other => other?,
        };
        let message = reply
            .pointer("/choices/0/message")
            .cloned()
            .ok_or_else(|| "The model returned an empty response".to_string())?;
        let tool_calls = message.get("tool_calls").and_then(|value| value.as_array()).cloned().unwrap_or_default();
        if tool_calls.is_empty() {
            let text = message.get("content").and_then(|value| value.as_str()).unwrap_or("").trim().to_string();
            if !native_tools {
                if let Some((name, args)) = parse_text_tool(&text) {
                    tools_used.push(name.clone());
                    let result = call_tool(&app, &name, args.clone()).await;
                    trace.push(tool_trace(&name, &args, &result));
                    if let Ok(value) = &result {
                        if let Some(action) = value.get("_action").cloned() {
                            actions.push(action);
                        }
                    }
                    let content = match result {
                        Ok(mut value) => {
                            value.as_object_mut().map(|object| object.remove("_action"));
                            value.to_string()
                        }
                        Err(error) => json!({ "error": error }).to_string(),
                    };
                    transcript.push(json!({ "role": "assistant", "content": text }));
                    transcript.push(json!({ "role": "user", "content": format!("Tool {name} returned: {content}") }));
                    continue;
                }
            }
            return Ok(AgentChatResponse {
                message: if text.is_empty() { "Done.".to_string() } else { text },
                actions,
                tools_used,
                trace,
            });
        }
        transcript.push(message);
        for call in tool_calls {
            let id = call.get("id").and_then(|value| value.as_str()).unwrap_or("call").to_string();
            let name = call
                .pointer("/function/name")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = call.pointer("/function/arguments").and_then(|value| value.as_str()).unwrap_or("{}");
            let args = serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}));
            tools_used.push(name.clone());
            let result = call_tool(&app, &name, args.clone()).await;
            trace.push(tool_trace(&name, &args, &result));
            if let Ok(value) = &result {
                if let Some(action) = value.get("_action").cloned() {
                    actions.push(action);
                }
            }
            let content = match result {
                Ok(mut value) => {
                    value.as_object_mut().map(|object| object.remove("_action"));
                    value.to_string()
                }
                Err(error) => json!({ "error": error }).to_string(),
            };
            transcript.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": content,
            }));
        }
    }
    Ok(AgentChatResponse {
        message: "I stopped after several tool calls. Ask me to continue if you want the next step.".to_string(),
        actions,
        tools_used,
        trace,
    })
}

fn tool_trace(name: &str, args: &Value, result: &Result<Value, String>) -> Value {
    let (ok, summary) = match result {
        Err(error) => (false, trim_error(error)),
        Ok(value) => (true, summarize_tool_value(value)),
    };
    json!({
        "kind": "tool",
        "tool": name,
        "ok": ok,
        "arguments": args,
        "summary": summary,
    })
}

fn summarize_tool_value(value: &Value) -> String {
    if let Some(skill) = value.get("skill").and_then(|item| item.as_str()) {
        let steps = value
            .get("steps")
            .and_then(|item| item.as_array())
            .map(|steps| {
                steps
                    .iter()
                    .filter_map(|step| step.get("tool").and_then(|tool| tool.as_str()))
                    .collect::<Vec<_>>()
                    .join(" → ")
            })
            .unwrap_or_default();
        let count = value.get("fileCount").and_then(|item| item.as_u64()).unwrap_or(0);
        return format!("{skill}: {steps} ({count} photos)");
    }
    if let Some(count) = value.get("fileCount").and_then(|item| item.as_u64()) {
        return format!("{count} photos");
    }
    if let Some(files) = value.get("files").and_then(|item| item.as_array()) {
        return format!("{} photos", files.len());
    }
    if let Some(completed) = value.get("completed").and_then(|item| item.as_u64()) {
        let errors = value.get("errors").and_then(|item| item.as_array()).map(|items| items.len()).unwrap_or(0);
        return format!("{completed} done, {errors} errors");
    }
    trim_error(&value.to_string())
}

fn system_prompt(context: &AgentContext) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d");
    let selected = if context.file_ids.is_empty() {
        "none".to_string()
    } else {
        context
            .file_ids
            .iter()
            .take(40)
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let focused = context.focused_file_id.map(|id| id.to_string()).unwrap_or_else(|| "none".to_string());
    let album = context.album_name.clone().unwrap_or_else(|| "none".to_string());
    let skills = skill_catalog();
    format!(
        "You are Lap, a local photo library assistant. Today is {today}. \
Use tools for anything about the user's photos. Do not invent file ids, titles, or counts. \
Albums are folders on disk. Collections are manual groups of photos. Smart albums are saved filters. \
search_photos matches file names and comments only, not keywords. \
query_photos is the fast metadata search: keyword, camera, lens, place, person, rating, culling (unreviewed, picked, or rejected), album, collection, and exact takenFrom/takenTo datetimes. \
photos_around finds photos taken within N minutes of a file or of photos matching a keyword. Use it for requests like \"within 5 minutes of the Monic photo\". Never invent a clock time. \
list_cameras, list_lenses, and list_locations return the real names in the library. \
search_metadata matches IPTC/XMP titles, captions, keywords, and places. \
update_metadata writes title, caption, headline, keywords, creator, copyright, city, state, country, rating, and GPS into the photo as IPTC and XMP (a sidecar for RAW) and updates Lap. \
copy_files, move_files, and delete_files work on file ids, a collection, or an album. delete_files moves files to the trash unless permanently is true. \
Keywords you write also become Lap tags. Removing a keyword detaches that tag. set_rating only changes Lap's stars and does not write the file. \
When the user wants to see a lasting filter, call create_smart_album. When they want to change the selected photos, use the selected file ids. \
Prefer run_skill when a workflow matches. Pass only its inputs; the skill calls the tools itself. Do not repeat the skill steps in your reply. \
Skills: {skills}. \
Selected file ids: {selected}. Focused file id: {focused}. Current album: {album}."
    )
}

async fn chat_completion(
    client: &reqwest::Client,
    settings: &AgentSettings,
    messages: &[Value],
    tools: Option<&Value>,
) -> Result<Value, String> {
    let url = format!("{}/chat/completions", settings.base_url.trim_end_matches('/'));
    let mut body = json!({
        "model": settings.model,
        "messages": messages,
        "temperature": 0.2,
    });
    if let Some(tools) = tools {
        body["tools"] = tools.clone();
    }
    let payload = serde_json::to_vec(&body).map_err(|error| error.to_string())?;
    let mut request = client
        .post(url)
        .header("content-type", "application/json")
        .body(payload);
    if settings.provider == "openrouter" || settings.base_url.contains("openrouter.ai") {
        request = request.header("HTTP-Referer", "https://julyx10.github.io/lap").header("X-Title", "Lap");
    }
    if !settings.api_key.is_empty() {
        request = request.bearer_auth(&settings.api_key);
    }
    let response = request.send().await.map_err(|error| format!("The model request failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        if missing_tool_endpoint(&body) {
            return Err(format!("tool-endpoint-missing: {}", trim_error(&body)));
        }
        return Err(format!("The model request failed ({status}): {}", trim_error(&body)));
    }
    serde_json::from_str(&body).map_err(|error| format!("The model returned invalid JSON: {error}"))
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| error.to_string())
}

fn tool_definitions() -> Value {
    json!([
        tool("list_skills", "List reusable workflows. Each skill runs its tools directly.", json!({"type":"object","properties":{}})),
        tool("run_skill", "Run a saved workflow by name. Pass inputs only. The skill calls the library tools itself.", json!({"type":"object","properties":{"name":{"type":"string"},"inputs":{"type":"object"}},"required":["name"]})),
        tool("save_skill", "Save a reusable workflow of tool steps. arguments may use {{input}} and {{stepId.fileIds}}.", json!({"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"inputs":{"type":"array"},"steps":{"type":"array"}},"required":["name","description","steps"]})),
        tool("list_albums", "List library albums (folders).", json!({"type":"object","properties":{}})),
        tool("add_album", "Add an existing folder as an album.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})),
        tool("list_collections", "List collections.", json!({"type":"object","properties":{}})),
        tool("create_collection", "Create a collection.", json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})),
        tool("add_to_collection", "Add photos to a collection by id or name.", json!({"type":"object","properties":{"collectionId":{"type":"integer"},"name":{"type":"string"},"fileIds":{"type":"array","items":{"type":"integer"}}},"required":["fileIds"]})),
        tool("search_photos", "Find photos by file name or comment. Does not search keywords, camera, or capture time.", json!({"type":"object","properties":{"text":{"type":"string"},"favorite":{"type":"boolean"},"rating":{"type":"integer"},"ratingMin":{"type":"integer"},"fileType":{"type":"string","enum":["image","video","audio"]},"make":{"type":"string"},"model":{"type":"string"},"tag":{"type":"string"},"person":{"type":"string"},"takenAfter":{"type":"string","description":"Calendar date YYYY-MM-DD, not a time of day"},"takenBefore":{"type":"string"},"limit":{"type":"integer"}}})),
        tool("query_photos", "Fast metadata query. keyword matches Lap tags and IPTC/XMP keywords. camera, lens, and place match those fields. culling is unreviewed, picked, or rejected. takenFrom and takenTo are local datetimes (YYYY-MM-DD or YYYY-MM-DD HH:MM:SS).", json!({"type":"object","properties":{"keyword":{"type":"string"},"camera":{"type":"string"},"lens":{"type":"string"},"place":{"type":"string"},"person":{"type":"string"},"text":{"type":"string","description":"Title, caption, or file name"},"favorite":{"type":"boolean"},"ratingMin":{"type":"integer"},"culling":{"type":"string","enum":["unreviewed","picked","rejected"]},"fileType":{"type":"string","enum":["image","video","audio"]},"albumId":{"type":"integer"},"collectionId":{"type":"integer"},"takenFrom":{"type":"string"},"takenTo":{"type":"string"},"limit":{"type":"integer"},"offset":{"type":"integer"}}})),
        tool("photos_around", "Photos taken within minutes of a photo or of photos matching a keyword. Uses each photo's capture time. minutes defaults to 5.", json!({"type":"object","properties":{"fileId":{"type":"integer"},"keyword":{"type":"string"},"minutes":{"type":"integer"},"limit":{"type":"integer"}},"required":[]})),
        tool("list_cameras", "Distinct camera make and model values in the library.", json!({"type":"object","properties":{"limit":{"type":"integer"}}})),
        tool("list_lenses", "Distinct lens make and model values in the library.", json!({"type":"object","properties":{"limit":{"type":"integer"}}})),
        tool("list_locations", "Distinct places stored on photos.", json!({"type":"object","properties":{"limit":{"type":"integer"}}})),
        tool("create_smart_album", "Save a filter as a smart album and show it. Rules use field, operator, and value.", json!({"type":"object","properties":{"name":{"type":"string"},"match":{"type":"string","enum":["all","any"]},"rules":{"type":"array","items":{"type":"object"}}},"required":["name","rules"]})),
        tool("search_metadata", "Find photos by title, caption, keyword, or place stored in IPTC/XMP.", json!({"type":"object","properties":{"text":{"type":"string"},"keyword":{"type":"string"},"place":{"type":"string"},"limit":{"type":"integer"}}})),
        tool("get_metadata", "Read title, caption, keywords, location, creator, and rating for photos.", json!({"type":"object","properties":{"fileIds":{"type":"array","items":{"type":"integer"}}},"required":["fileIds"]})),
        tool("update_metadata", "Write IPTC/XMP title, caption, headline, keywords, creator, copyright, credit, city, state, country, rating, or GPS. Also updates Lap tags for keywords.", json!({"type":"object","properties":{"fileIds":{"type":"array","items":{"type":"integer"}},"title":{"type":"string"},"headline":{"type":"string"},"description":{"type":"string"},"creator":{"type":"string"},"copyright":{"type":"string"},"credit":{"type":"string"},"city":{"type":"string"},"state":{"type":"string"},"country":{"type":"string"},"label":{"type":"string"},"rating":{"type":"integer"},"latitude":{"type":"number"},"longitude":{"type":"number"},"keywords":{"type":"array","items":{"type":"string"}},"addKeywords":{"type":"array","items":{"type":"string"}},"removeKeywords":{"type":"array","items":{"type":"string"}},"clear":{"type":"array","items":{"type":"string"}}},"required":["fileIds"]})),
        tool("set_rating", "Set Lap stars from 0 to 5 without writing the file.", json!({"type":"object","properties":{"fileIds":{"type":"array","items":{"type":"integer"}},"rating":{"type":"integer"}},"required":["fileIds","rating"]})),
        tool("set_favorite", "Mark or unmark Lap favorites.", json!({"type":"object","properties":{"fileIds":{"type":"array","items":{"type":"integer"}},"favorite":{"type":"boolean"}},"required":["fileIds","favorite"]})),
        tool("list_tags", "List Lap tags.", json!({"type":"object","properties":{}})),
        tool("tag_files", "Attach an existing or new Lap tag.", json!({"type":"object","properties":{"name":{"type":"string"},"fileIds":{"type":"array","items":{"type":"integer"}}},"required":["name","fileIds"]})),
        tool("import_files", "Copy photos into an album folder that is already in the library.", json!({"type":"object","properties":{"destinationPath":{"type":"string"},"sourcePaths":{"type":"array","items":{"type":"string"}}},"required":["destinationPath","sourcePaths"]})),
        tool("copy_files", "Copy a batch of photos, a collection, or an album into a folder. Creates the folder if its parent exists. Files copied into a library folder are added to the catalog.", json!({"type":"object","properties":{"destinationPath":{"type":"string"},"fileIds":{"type":"array","items":{"type":"integer"}},"collectionId":{"type":"integer"},"albumId":{"type":"integer"},"limit":{"type":"integer"},"offset":{"type":"integer"}},"required":["destinationPath"]})),
        tool("move_files", "Move a batch of photos, a collection, or an album into a folder. Moving into a library folder keeps them in Lap. Moving elsewhere removes them from the catalog.", json!({"type":"object","properties":{"destinationPath":{"type":"string"},"fileIds":{"type":"array","items":{"type":"integer"}},"collectionId":{"type":"integer"},"albumId":{"type":"integer"},"limit":{"type":"integer"},"offset":{"type":"integer"}},"required":["destinationPath"]})),
        tool("delete_files", "Move photos to the trash. Pass permanently=true only when the user explicitly wants them erased. Accepts file ids, a collection, or an album.", json!({"type":"object","properties":{"fileIds":{"type":"array","items":{"type":"integer"}},"collectionId":{"type":"integer"},"albumId":{"type":"integer"},"permanently":{"type":"boolean"},"limit":{"type":"integer"},"offset":{"type":"integer"}}})),
        tool("start_index", "Index an album so new files and metadata appear in Lap.", json!({"type":"object","properties":{"albumId":{"type":"integer"}},"required":["albumId"]})),
        tool("list_people", "List recognized people.", json!({"type":"object","properties":{}}))
    ])
}

fn tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": { "name": name, "description": description, "parameters": parameters }
    })
}

async fn call_tool(app: &AppHandle, name: &str, args: Value) -> Result<Value, String> {
    let app = app.clone();
    let name = name.to_string();
    tokio::task::spawn_blocking(move || dispatch_tool(&app, &name, &args))
        .await
        .map_err(|error| error.to_string())?
}

fn dispatch_tool(app: &AppHandle, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "list_skills" => Ok(json!({ "skills": list_skill_views() })),
        "run_skill" => run_skill(app, args),
        "save_skill" => save_skill(args),
        "list_albums" => {
            let albums = Album::get_all_albums()?
                .into_iter()
                .map(|album| json!({"id": album.id, "name": album.name, "path": album.path, "total": album.total}))
                .collect::<Vec<_>>();
            Ok(json!({ "albums": albums }))
        }
        "add_album" => {
            let path = required_str(args, "path")?;
            let album = crate::t_cmds::add_album(app.clone(), &path)?;
            let _ = app.emit("albums-changed", json!({ "albumId": album.id }));
            Ok(json!({ "album": { "id": album.id, "name": album.name, "path": album.path } }))
        }
        "list_collections" => {
            let collections = ACollection::list()?;
            Ok(json!({ "collections": collections }))
        }
        "create_collection" => {
            let name = required_str(args, "name")?;
            let collection = ACollection::create(&name)?;
            let _ = app.emit("collections-changed", json!({ "collectionId": collection.id }));
            Ok(json!({ "collection": collection }))
        }
        "add_to_collection" => {
            let file_ids = required_ids(args, "fileIds")?;
            let collection_id = collection_id(args)?;
            let (added, skipped) = ACollection::add_files(collection_id, file_ids)?;
            let _ = app.emit("collections-changed", json!({ "collectionId": collection_id }));
            Ok(json!({ "collectionId": collection_id, "added": added.len(), "skipped": skipped.len() }))
        }
        "search_photos" => search_photos(args),
        "query_photos" => query_photos(args),
        "photos_around" => photos_around(args),
        "list_cameras" => list_grouped("camera", args),
        "list_lenses" => list_grouped("lens", args),
        "list_locations" => list_grouped("location", args),
        "create_smart_album" => create_smart_album(app, args),
        "search_metadata" => crate::t_cmds::search_file_metadata(
            args.get("text").and_then(|value| value.as_str()).map(str::to_string),
            args.get("keyword").and_then(|value| value.as_str()).map(str::to_string),
            args.get("place").and_then(|value| value.as_str()).map(str::to_string),
            args.get("limit").and_then(|value| value.as_i64()),
        )
        .map(|files| json!({ "files": files })),
        "get_metadata" => crate::t_cmds::get_files_metadata(required_ids(args, "fileIds")?).map(|files| json!({ "files": files })),
        "update_metadata" => {
            let file_ids = required_ids(args, "fileIds")?;
            let change: crate::t_file_metadata::MetadataChange = serde_json::from_value(args.clone()).map_err(|error| error.to_string())?;
            let files = crate::t_cmds::update_files_metadata(app.clone(), file_ids, change)?;
            Ok(json!({ "files": files, "_action": { "type": "metadata_updated", "fileIds": files.iter().filter_map(|file| file.get("fileId").and_then(|id| id.as_i64())).collect::<Vec<_>>() } }))
        }
        "set_rating" => {
            let rating = args.get("rating").and_then(|value| value.as_i64()).ok_or("Rating is required")?.clamp(0, 5) as i32;
            let file_ids = required_ids(args, "fileIds")?;
            for file_id in &file_ids {
                AFile::update_column(*file_id, "rating", &rating)?;
            }
            Ok(json!({ "updated": file_ids.len(), "rating": rating }))
        }
        "set_favorite" => {
            let favorite = args.get("favorite").and_then(|value| value.as_bool()).unwrap_or(true);
            let file_ids = required_ids(args, "fileIds")?;
            for file_id in &file_ids {
                AFile::update_column(*file_id, "is_favorite", &favorite)?;
            }
            Ok(json!({ "updated": file_ids.len(), "favorite": favorite }))
        }
        "list_tags" => {
            let tags = ATag::get_all(0, 0)?
                .into_iter()
                .map(|tag| json!({"id": tag.id, "name": tag.name}))
                .collect::<Vec<_>>();
            Ok(json!({ "tags": tags }))
        }
        "tag_files" => {
            let name = required_str(args, "name")?;
            let file_ids = required_ids(args, "fileIds")?;
            let tag_id = ensure_tag(&name)?;
            for file_id in &file_ids {
                ATag::add_tag_to_file(*file_id, tag_id)?;
            }
            let _ = app.emit("tags-changed", json!({ "tagId": tag_id }));
            Ok(json!({ "tagId": tag_id, "name": name, "updated": file_ids.len() }))
        }
        "import_files" => import_files(app, args),
        "copy_files" => transfer_files(app, args, TransferKind::Copy),
        "move_files" => transfer_files(app, args, TransferKind::Move),
        "delete_files" => transfer_files(app, args, TransferKind::Delete),
        "start_index" => {
            let album_id = args.get("albumId").and_then(|value| value.as_i64()).ok_or("albumId is required")?;
            start_index(app, album_id)?;
            Ok(json!({ "albumId": album_id, "started": true }))
        }
        "list_people" => list_people(),
        _ => Err(format!("Unknown tool: {name}")),
    }
}

#[derive(Clone, Copy)]
enum TransferKind {
    Copy,
    Move,
    Delete,
}

struct PhotoFilter {
    keyword: String,
    camera: String,
    lens: String,
    place: String,
    person: String,
    text: String,
    favorite: Option<bool>,
    rating_min: Option<i64>,
    culling: Option<i64>,
    file_type: i64,
    album_id: Option<i64>,
    collection_id: Option<i64>,
    taken_from: Option<i64>,
    taken_to: Option<i64>,
    limit: i64,
    offset: i64,
}

fn query_photos(args: &Value) -> Result<Value, String> {
    let filter = photo_filter_from_args(args)?;
    let conn = crate::t_sqlite::open_conn()?;
    let files = query_photos_on(&conn, &filter)?;
    let ids = files.iter().filter_map(|file| file.get("fileId").and_then(|id| id.as_i64())).collect::<Vec<_>>();
    Ok(json!({
        "total": ids.len(),
        "files": files,
        "_action": { "type": "show_files", "title": filter.keyword, "fileIds": ids }
    }))
}

fn photos_around(args: &Value) -> Result<Value, String> {
    let minutes = args.get("minutes").and_then(|value| value.as_i64()).unwrap_or(5).clamp(1, 24 * 60);
    let file_id = args.get("fileId").and_then(|value| value.as_i64()).unwrap_or(0);
    let keyword = args.get("keyword").and_then(|value| value.as_str()).unwrap_or("").trim().to_string();
    if file_id <= 0 && keyword.is_empty() {
        return Err("Provide fileId or keyword".to_string());
    }
    let limit = args.get("limit").and_then(|value| value.as_i64()).unwrap_or(40).clamp(1, 200);
    let conn = crate::t_sqlite::open_conn()?;
    let files = photos_around_on(&conn, file_id, &keyword, minutes * 60, limit)?;
    let ids = files.iter().filter_map(|file| file.get("fileId").and_then(|id| id.as_i64())).collect::<Vec<_>>();
    Ok(json!({
        "minutes": minutes,
        "anchorCount": files.iter().filter_map(|file| file.get("anchor").and_then(|value| value.as_bool())).filter(|value| *value).count(),
        "files": files,
        "_action": { "type": "show_files", "title": keyword, "fileIds": ids }
    }))
}

fn photo_filter_from_args(args: &Value) -> Result<PhotoFilter, String> {
    Ok(PhotoFilter {
        keyword: arg_text(args, "keyword"),
        camera: arg_text(args, "camera"),
        lens: arg_text(args, "lens"),
        place: arg_text(args, "place"),
        person: arg_text(args, "person"),
        text: arg_text(args, "text"),
        favorite: args.get("favorite").and_then(|value| value.as_bool()),
        rating_min: args.get("ratingMin").and_then(|value| value.as_i64()),
        culling: args.get("culling").and_then(parse_culling),
        file_type: match args.get("fileType").and_then(|value| value.as_str()).unwrap_or("") {
            "image" => 1,
            "video" => 2,
            "audio" => 3,
            _ => 0,
        },
        album_id: args.get("albumId").and_then(|value| value.as_i64()).filter(|id| *id > 0),
        collection_id: args.get("collectionId").and_then(|value| value.as_i64()).filter(|id| *id > 0),
        taken_from: args.get("takenFrom").and_then(|value| value.as_str()).and_then(parse_local_datetime),
        taken_to: args.get("takenTo").and_then(|value| value.as_str()).and_then(parse_local_datetime),
        limit: args.get("limit").and_then(|value| value.as_i64()).unwrap_or(40).clamp(1, 200),
        offset: args.get("offset").and_then(|value| value.as_i64()).unwrap_or(0).max(0),
    })
}

fn query_photos_on(conn: &rusqlite::Connection, filter: &PhotoFilter) -> Result<Vec<Value>, String> {
    let mut sql = String::from(
        "SELECT a.id, a.name, b.path, IFNULL(a.taken_date, 0), IFNULL(a.e_make, ''), IFNULL(a.e_model, ''),
                IFNULL(a.e_lens_model, ''), IFNULL(a.e_title, ''), IFNULL(a.rating, 0),
                IFNULL(a.geo_name, ''), IFNULL(a.e_location, '')
         FROM afiles a
         JOIN afolders b ON b.id = a.folder_id
         WHERE a.id NOT IN (SELECT live_photo_video_id FROM afiles WHERE live_photo_video_id IS NOT NULL)",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    push_like(&mut sql, &mut params, &filter.keyword, "(EXISTS (SELECT 1 FROM afile_tags ft JOIN atags t ON t.id = ft.tag_id WHERE ft.file_id = a.id AND t.name LIKE ? ESCAPE '\\' COLLATE NOCASE) OR a.e_keywords LIKE ? ESCAPE '\\' COLLATE NOCASE)");
    if !filter.keyword.is_empty() {
        let pattern = like_contains(&filter.keyword);
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
    }
    push_like(&mut sql, &mut params, &filter.camera, "(a.e_make LIKE ? ESCAPE '\\' COLLATE NOCASE OR a.e_model LIKE ? ESCAPE '\\' COLLATE NOCASE OR (a.e_make || ' ' || a.e_model) LIKE ? ESCAPE '\\' COLLATE NOCASE)");
    if !filter.camera.is_empty() {
        let pattern = like_contains(&filter.camera);
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
    }
    push_like(&mut sql, &mut params, &filter.lens, "(a.e_lens_make LIKE ? ESCAPE '\\' COLLATE NOCASE OR a.e_lens_model LIKE ? ESCAPE '\\' COLLATE NOCASE OR (IFNULL(a.e_lens_make, '') || ' ' || IFNULL(a.e_lens_model, '')) LIKE ? ESCAPE '\\' COLLATE NOCASE)");
    if !filter.lens.is_empty() {
        let pattern = like_contains(&filter.lens);
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
    }
    push_like(&mut sql, &mut params, &filter.place, "(a.e_location LIKE ? ESCAPE '\\' COLLATE NOCASE OR a.geo_name LIKE ? ESCAPE '\\' COLLATE NOCASE OR a.geo_admin1 LIKE ? ESCAPE '\\' COLLATE NOCASE OR a.geo_cc LIKE ? ESCAPE '\\' COLLATE NOCASE)");
    if !filter.place.is_empty() {
        let pattern = like_contains(&filter.place);
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
    }
    push_like(&mut sql, &mut params, &filter.person, "EXISTS (SELECT 1 FROM faces f JOIN persons p ON p.id = f.person_id WHERE f.file_id = a.id AND p.name LIKE ? ESCAPE '\\' COLLATE NOCASE)");
    if !filter.person.is_empty() {
        params.push(Box::new(like_contains(&filter.person)));
    }
    push_like(&mut sql, &mut params, &filter.text, "(a.name LIKE ? ESCAPE '\\' COLLATE NOCASE OR a.e_title LIKE ? ESCAPE '\\' COLLATE NOCASE OR a.e_description LIKE ? ESCAPE '\\' COLLATE NOCASE OR a.comments LIKE ? ESCAPE '\\' COLLATE NOCASE)");
    if !filter.text.is_empty() {
        let pattern = like_contains(&filter.text);
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
    }
    if let Some(favorite) = filter.favorite {
        if favorite {
            sql.push_str(" AND a.is_favorite = 1");
        }
    }
    if let Some(rating) = filter.rating_min {
        sql.push_str(" AND IFNULL(a.rating, 0) >= ?");
        params.push(Box::new(rating));
    }
    if let Some(culling) = filter.culling {
        sql.push_str(" AND IFNULL(a.culling_flag, 0) = ?");
        params.push(Box::new(culling));
    }
    if filter.file_type > 0 {
        sql.push_str(" AND a.file_type = ?");
        params.push(Box::new(filter.file_type));
    }
    if let Some(album_id) = filter.album_id {
        sql.push_str(" AND b.album_id = ?");
        params.push(Box::new(album_id));
    }
    if let Some(collection_id) = filter.collection_id {
        sql.push_str(" AND EXISTS (SELECT 1 FROM acollections_files cf WHERE cf.file_id = a.id AND cf.collection_id = ?)");
        params.push(Box::new(collection_id));
    }
    if let Some(taken_from) = filter.taken_from {
        sql.push_str(" AND a.taken_date >= ?");
        params.push(Box::new(taken_from));
    }
    if let Some(taken_to) = filter.taken_to {
        sql.push_str(" AND a.taken_date <= ?");
        params.push(Box::new(taken_to));
    }
    sql.push_str(" ORDER BY a.taken_date DESC, a.id DESC LIMIT ? OFFSET ?");
    params.push(Box::new(filter.limit));
    params.push(Box::new(filter.offset));
    let values: Vec<&dyn rusqlite::ToSql> = params.iter().map(|value| value.as_ref()).collect();
    let mut stmt = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(values.as_slice(), map_photo_row)
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())
}

fn push_like(sql: &mut String, _params: &mut Vec<Box<dyn rusqlite::ToSql>>, value: &str, clause: &str) {
    if value.trim().is_empty() {
        return;
    }
    sql.push_str(" AND ");
    sql.push_str(clause);
}

fn photos_around_on(
    conn: &rusqlite::Connection,
    file_id: i64,
    keyword: &str,
    seconds: i64,
    limit: i64,
) -> Result<Vec<Value>, String> {
    let pattern = like_contains(keyword);
    let mut stmt = conn
        .prepare(
            "WITH anchors AS (
                SELECT a.id, a.taken_date
                FROM afiles a
                WHERE IFNULL(a.taken_date, 0) > 0
                  AND (
                    (?1 > 0 AND a.id = ?1)
                    OR (?1 <= 0 AND ?2 != '' AND (
                      EXISTS (SELECT 1 FROM afile_tags ft JOIN atags t ON t.id = ft.tag_id
                              WHERE ft.file_id = a.id AND t.name LIKE ?3 ESCAPE '\\' COLLATE NOCASE)
                      OR IFNULL(a.e_keywords, '') LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                    ))
                  )
                LIMIT 20
             )
             SELECT n.id, n.name, b.path, n.taken_date, IFNULL(n.e_make, ''), IFNULL(n.e_model, ''),
                    IFNULL(n.e_lens_model, ''), IFNULL(n.e_title, ''), IFNULL(n.rating, 0),
                    IFNULL(n.geo_name, ''), IFNULL(n.e_location, ''), c.id,
                    ABS(n.taken_date - c.taken_date)
             FROM anchors c
             JOIN afiles n ON n.taken_date BETWEEN c.taken_date - ?4 AND c.taken_date + ?4
             JOIN afolders b ON b.id = n.folder_id
             WHERE n.id NOT IN (SELECT live_photo_video_id FROM afiles WHERE live_photo_video_id IS NOT NULL)
             ORDER BY c.taken_date, ABS(n.taken_date - c.taken_date), n.id
             LIMIT ?5",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![file_id, keyword, pattern, seconds, limit], |row| {
            let mut file = map_photo_row(row)?;
            let anchor_id: i64 = row.get(11)?;
            let delta: i64 = row.get(12)?;
            if let Some(object) = file.as_object_mut() {
                object.insert("anchorFileId".to_string(), json!(anchor_id));
                object.insert("secondsFromAnchor".to_string(), json!(delta));
                object.insert("anchor".to_string(), json!(row.get::<_, i64>(0)? == anchor_id));
            }
            Ok(file)
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())
}

fn map_photo_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let name: String = row.get(1)?;
    let folder: String = row.get(2)?;
    let taken: i64 = row.get(3)?;
    let camera = format!("{} {}", row.get::<_, String>(4)?, row.get::<_, String>(5)?);
    let geo: String = row.get(9)?;
    let location: String = row.get(10)?;
    Ok(json!({
        "fileId": row.get::<_, i64>(0)?,
        "name": name,
        "path": crate::t_utils::get_file_path(&folder, &name),
        "takenUnix": taken,
        "taken": format_taken(taken),
        "camera": camera.trim(),
        "lens": row.get::<_, String>(6)?,
        "title": row.get::<_, String>(7)?,
        "rating": row.get::<_, i64>(8)?,
        "place": if location.is_empty() { geo } else { location },
    }))
}

fn list_grouped(kind: &str, args: &Value) -> Result<Value, String> {
    let limit = args.get("limit").and_then(|value| value.as_i64()).unwrap_or(80).clamp(1, 200);
    let (sql, label) = match kind {
        "camera" => (
            "SELECT TRIM(IFNULL(e_make, '') || ' ' || IFNULL(e_model, '')) AS label, COUNT(*)
             FROM afiles
             WHERE TRIM(IFNULL(e_make, '') || IFNULL(e_model, '')) != ''
             GROUP BY label ORDER BY COUNT(*) DESC LIMIT ?1",
            "cameras",
        ),
        "lens" => (
            "SELECT TRIM(IFNULL(e_lens_make, '') || ' ' || IFNULL(e_lens_model, '')) AS label, COUNT(*)
             FROM afiles
             WHERE TRIM(IFNULL(e_lens_make, '') || IFNULL(e_lens_model, '')) != ''
             GROUP BY label ORDER BY COUNT(*) DESC LIMIT ?1",
            "lenses",
        ),
        _ => (
            "SELECT TRIM(COALESCE(NULLIF(e_location, ''), NULLIF(geo_name, ''), NULLIF(geo_admin1, ''), geo_cc)) AS label, COUNT(*)
             FROM afiles
             WHERE TRIM(COALESCE(NULLIF(e_location, ''), NULLIF(geo_name, ''), NULLIF(geo_admin1, ''), IFNULL(geo_cc, ''))) != ''
             GROUP BY label ORDER BY COUNT(*) DESC LIMIT ?1",
            "locations",
        ),
    };
    let conn = crate::t_sqlite::open_conn()?;
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![limit], |row| {
            Ok(json!({ "name": row.get::<_, String>(0)?, "count": row.get::<_, i64>(1)? }))
        })
        .map_err(|error| error.to_string())?;
    let items = rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
    Ok(json!({ label: items }))
}

fn transfer_files(app: &AppHandle, args: &Value, kind: TransferKind) -> Result<Value, String> {
    let destination = args.get("destinationPath").and_then(|value| value.as_str()).unwrap_or("").trim().to_string();
    if !matches!(kind, TransferKind::Delete) && destination.is_empty() {
        return Err("destinationPath is required".to_string());
    }
    let limit = args.get("limit").and_then(|value| value.as_i64()).unwrap_or(200).clamp(1, 1000);
    let offset = args.get("offset").and_then(|value| value.as_i64()).unwrap_or(0).max(0);
    let permanently = args.get("permanently").and_then(|value| value.as_bool()).unwrap_or(false);
    let ids = resolve_transfer_ids(args, limit, offset)?;
    if ids.is_empty() {
        return Err("No photos matched. Pass fileIds, collectionId, or albumId.".to_string());
    }
    if !destination.is_empty() {
        let path = Path::new(&destination);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() && !path.exists() {
                return Err(format!("Parent folder does not exist: {}", parent.display()));
            }
        }
        std::fs::create_dir_all(path).map_err(|error| format!("Could not create folder: {error}"))?;
    }
    let library_folder = if destination.is_empty() { None } else { library_folder(&destination)? };
    let mut done = Vec::new();
    let mut errors = Vec::new();
    for file_id in ids {
        let Some((path, _folder_id)) = file_path_and_folder(file_id)? else {
            errors.push(format!("{file_id}: not in the library"));
            continue;
        };
        let result = match kind {
            TransferKind::Copy => {
                if let Some((folder_id, folder_path, album_id)) = &library_folder {
                    match crate::t_cmds::import_file(&path, *folder_id, folder_path) {
                        Ok(Some(file)) => {
                            let _ = app.emit("import-files-added", json!({ "albumId": album_id }));
                            Ok(file.id.unwrap_or(0).to_string())
                        }
                        Ok(None) => Err("skipped".to_string()),
                        Err(error) => Err(error),
                    }
                } else {
                    crate::t_cmds::copy_file(&path, &destination, "keep")
                }
            }
            TransferKind::Move => {
                if let Some((folder_id, folder_path, _)) = &library_folder {
                    crate::t_cmds::move_file(file_id, &path, *folder_id, folder_path, "keep")
                } else {
                    crate::t_cmds::move_file_outside_library(file_id, &path, &destination, "keep")
                }
            }
            TransferKind::Delete => {
                let deleted = if permanently {
                    crate::t_cmds::delete_file_permanently(file_id, &path)
                } else {
                    crate::t_cmds::delete_file(file_id, &path)
                };
                deleted.map(|result| result.deleted_file_ids.len().to_string())
            }
        };
        match result {
            Ok(value) => done.push(json!({ "fileId": file_id, "result": value })),
            Err(error) => errors.push(format!("{file_id}: {error}")),
        }
    }
    if matches!(kind, TransferKind::Move | TransferKind::Delete) || library_folder.is_some() {
        let _ = app.emit("library-total-refreshed", json!({ "source": "agent" }));
    }
    Ok(json!({
        "completed": done.len(),
        "errors": errors,
        "permanently": permanently,
    }))
}

fn resolve_transfer_ids(args: &Value, limit: i64, offset: i64) -> Result<Vec<i64>, String> {
    if let Some(ids) = args.get("fileIds").and_then(|value| value.as_array()) {
        return Ok(ids.iter().filter_map(|value| value.as_i64()).filter(|id| *id > 0).skip(offset as usize).take(limit as usize).collect());
    }
    if let Some(collection_id) = args.get("collectionId").and_then(|value| value.as_i64()) {
        let mut ids = ACollection::file_ids(collection_id)?;
        ids.sort_unstable();
        return Ok(ids.into_iter().skip(offset as usize).take(limit as usize).collect());
    }
    if let Some(album_id) = args.get("albumId").and_then(|value| value.as_i64()) {
        let conn = crate::t_sqlite::open_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT a.id FROM afiles a JOIN afolders b ON b.id = a.folder_id
                 WHERE b.album_id = ?1 ORDER BY a.taken_date, a.id LIMIT ?2 OFFSET ?3",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![album_id, limit, offset], |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())?;
        return rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string());
    }
    Ok(Vec::new())
}

fn library_folder(path: &str) -> Result<Option<(i64, String, i64)>, String> {
    let conn = crate::t_sqlite::open_conn()?;
    let found = conn
        .query_row(
            "SELECT id, path, album_id FROM afolders WHERE path = ?1",
            rusqlite::params![path],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
        )
        .ok();
    Ok(found)
}

fn file_path_and_folder(file_id: i64) -> Result<Option<(String, i64)>, String> {
    let conn = crate::t_sqlite::open_conn()?;
    let found = conn
        .query_row(
            "SELECT a.name, b.path, b.id FROM afiles a JOIN afolders b ON b.id = a.folder_id WHERE a.id = ?1",
            rusqlite::params![file_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
        )
        .ok();
    Ok(found.map(|(name, folder, folder_id)| (crate::t_utils::get_file_path(&folder, &name), folder_id)))
}

fn parse_culling(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return matches!(number, 0..=2).then_some(number);
    }
    match value.as_str()?.trim().to_ascii_lowercase().as_str() {
        "unreviewed" | "unprocessed" | "none" | "0" => Some(0),
        "pick" | "picked" | "picks" | "1" => Some(1),
        "reject" | "rejected" | "2" => Some(2),
        _ => None,
    }
}

fn arg_text(args: &Value, key: &str) -> String {
    args.get(key).and_then(|value| value.as_str()).unwrap_or("").trim().to_string()
}

fn like_contains(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

fn parse_local_datetime(value: &str) -> Option<i64> {
    let value = value.trim();
    let naive = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .or_else(|_| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map(|date| date.and_hms_opt(0, 0, 0).unwrap_or_default()))
        .ok()?;
    chrono::Local.from_local_datetime(&naive).single().map(|time| time.timestamp())
}

fn format_taken(unix: i64) -> String {
    if unix <= 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|time| time.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn skill_catalog() -> String {
    list_skill_views()
        .into_iter()
        .map(|skill| {
            let name = skill["name"].as_str().unwrap_or("");
            let inputs = skill["inputs"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|input| {
                            let input_name = input["name"].as_str().unwrap_or("");
                            if input["required"].as_bool().unwrap_or(false) {
                                input_name.to_string()
                            } else {
                                format!("{input_name}?")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!("{name}({inputs})")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn list_skill_views() -> Vec<Value> {
    all_skills()
        .into_iter()
        .map(|skill| {
            json!({
                "name": skill["name"],
                "description": skill["description"],
                "inputs": skill["inputs"],
            })
        })
        .collect()
}

fn all_skills() -> Vec<Value> {
    let mut skills = builtin_skills();
    if let Ok(dir) = skills_dir() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&path) else { continue };
                let Ok(skill) = serde_json::from_str::<Value>(&text) else { continue };
                if skill.get("name").and_then(|value| value.as_str()).is_none() {
                    continue;
                }
                if let Some(name) = skill["name"].as_str() {
                    skills.retain(|existing| existing["name"].as_str() != Some(name));
                }
                skills.push(skill);
            }
        }
    }
    skills.sort_by(|left, right| left["name"].as_str().unwrap_or("").cmp(right["name"].as_str().unwrap_or("")));
    skills
}

fn builtin_skills() -> Vec<Value> {
    vec![
        skill("around-keyword", "Photos matching a keyword and photos taken within a few minutes of them.", &[("keyword", true, None), ("minutes", false, Some("5"))], &[("found", "photos_around", r#"{"keyword":"{{keyword}}","minutes":"{{minutes}}"}"#)]),
        skill("by-keyword", "Photos whose tag or keyword matches.", &[("keyword", true, None)], &[("found", "query_photos", r#"{"keyword":"{{keyword}}"}"#)]),
        skill("by-culling", "Photos that are unreviewed, picked, or rejected.", &[("culling", true, None), ("camera", false, None), ("lens", false, None), ("place", false, None), ("keyword", false, None)], &[("found", "query_photos", r#"{"culling":"{{culling}}","camera":"{{camera}}","lens":"{{lens}}","place":"{{place}}","keyword":"{{keyword}}"}"#)]),
        skill("by-camera", "Photos from a camera.", &[("camera", true, None)], &[("found", "query_photos", r#"{"camera":"{{camera}}"}"#)]),
        skill("by-lens", "Photos from a lens.", &[("lens", true, None)], &[("found", "query_photos", r#"{"lens":"{{lens}}"}"#)]),
        skill("by-place", "Photos from a place.", &[("place", true, None)], &[("found", "query_photos", r#"{"place":"{{place}}"}"#)]),
        skill("copy-album", "Copy an album into a folder.", &[("albumId", true, None), ("destinationPath", true, None)], &[("copied", "copy_files", r#"{"albumId":"{{albumId}}","destinationPath":"{{destinationPath}}"}"#)]),
        skill("copy-collection", "Copy a collection into a folder.", &[("collectionId", true, None), ("destinationPath", true, None)], &[("copied", "copy_files", r#"{"collectionId":"{{collectionId}}","destinationPath":"{{destinationPath}}"}"#)]),
        skill("move-album", "Move an album into a folder.", &[("albumId", true, None), ("destinationPath", true, None)], &[("moved", "move_files", r#"{"albumId":"{{albumId}}","destinationPath":"{{destinationPath}}"}"#)]),
        skill("trash-rejected", "Move rejected photos to the trash.", &[], &[("found", "query_photos", r#"{"culling":"rejected","limit":200}"#), ("trashed", "delete_files", r#"{"fileIds":"{{found.fileIds}}"}"#)]),
    ]
}

fn skill(name: &str, description: &str, inputs: &[(&str, bool, Option<&str>)], steps: &[(&str, &str, &str)]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputs": inputs.iter().map(|(input_name, required, default)| {
            let mut input = json!({ "name": input_name, "required": required });
            if let Some(default) = default {
                input["default"] = json!(default);
            }
            input
        }).collect::<Vec<_>>(),
        "steps": steps.iter().map(|(id, tool_name, arguments)| json!({
            "id": id,
            "tool": tool_name,
            "arguments": serde_json::from_str::<Value>(arguments).unwrap_or(json!({})),
        })).collect::<Vec<_>>(),
    })
}

fn run_skill(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let name = required_str(args, "name")?;
    let skill = all_skills()
        .into_iter()
        .find(|skill| skill["name"].as_str() == Some(name.as_str()))
        .ok_or_else(|| format!("Unknown skill: {name}"))?;
    let mut inputs = args.get("inputs").cloned().unwrap_or_else(|| json!({}));
    if let Some(definitions) = skill["inputs"].as_array() {
        for input in definitions {
            let input_name = input["name"].as_str().unwrap_or("");
            let missing = inputs.get(input_name).map(|value| value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty())).unwrap_or(true);
            if missing {
                if let Some(default) = input.get("default").filter(|value| !value.is_null()) {
                    inputs[input_name] = default.clone();
                } else if input["required"].as_bool().unwrap_or(false) {
                    return Err(format!("Skill {name} needs {input_name}"));
                }
            }
        }
    }
    let mut scope = inputs;
    let mut steps = Vec::new();
    let mut file_ids = Vec::new();
    for step in skill["steps"].as_array().cloned().unwrap_or_default() {
        let step_id = step["id"].as_str().unwrap_or("step").to_string();
        let tool_name = step["tool"].as_str().unwrap_or("").to_string();
        ensure_skill_tool(&tool_name)?;
        let arguments = resolve_value(step.get("arguments").unwrap_or(&json!({})), &scope);
        let result = dispatch_tool(app, &tool_name, &arguments)?;
        if let Some(ids) = result.pointer("/_action/fileIds").and_then(|value| value.as_array()) {
            file_ids = ids.iter().filter_map(|id| id.as_i64()).collect();
        }
        let output = step_output(&result);
        steps.push(json!({ "id": step_id, "tool": tool_name, "output": output }));
        scope[&step_id] = output;
    }
    Ok(json!({
        "skill": name,
        "fileCount": file_ids.len(),
        "fileIds": file_ids.iter().take(12).copied().collect::<Vec<_>>(),
        "steps": steps.iter().map(|step| {
            let mut copy = step.clone();
            let preview = copy
                .pointer("/output/fileIds")
                .and_then(|value| value.as_array())
                .map(|ids| (ids.len(), ids.iter().take(8).cloned().collect::<Vec<_>>()));
            if let Some((count, ids)) = preview {
                copy["output"]["fileCount"] = json!(count);
                copy["output"]["fileIds"] = json!(ids);
            }
            copy
        }).collect::<Vec<_>>(),
        "_action": { "type": "show_files", "title": name, "fileIds": file_ids },
    }))
}

fn step_output(result: &Value) -> Value {
    let mut output = json!({});
    if let Some(object) = result.as_object() {
        for (key, value) in object {
            if key == "_action" || key == "files" {
                continue;
            }
            output[key] = value.clone();
        }
    }
    let ids = result
        .pointer("/_action/fileIds")
        .cloned()
        .or_else(|| result.get("fileIds").cloned())
        .unwrap_or_else(|| json!([]));
    output["fileIds"] = ids;
    output["fileCount"] = json!(output["fileIds"].as_array().map(|items| items.len()).unwrap_or(0));
    output
}

fn save_skill(args: &Value) -> Result<Value, String> {
    let name = required_str(args, "name")?;
    if !is_skill_name(&name) {
        return Err("Skill name must be lowercase letters, numbers, and hyphens".to_string());
    }
    let description = required_str(args, "description")?;
    let steps = args.get("steps").and_then(|value| value.as_array()).cloned().ok_or("steps are required")?;
    if steps.is_empty() || steps.len() > 12 {
        return Err("A skill needs 1 to 12 steps".to_string());
    }
    for step in &steps {
        let tool_name = step["tool"].as_str().unwrap_or("");
        ensure_skill_tool(tool_name)?;
        if !step["arguments"].is_object() && !step.get("arguments").is_none() {
            return Err("Each step arguments value must be an object".to_string());
        }
    }
    let skill = json!({
        "name": name,
        "description": description,
        "inputs": args.get("inputs").cloned().unwrap_or_else(|| json!([])),
        "steps": steps,
    });
    let dir = skills_dir()?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join(format!("{name}.json"));
    fs::write(&path, serde_json::to_string_pretty(&skill).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    Ok(json!({ "saved": name, "path": path.display().to_string() }))
}

fn ensure_skill_tool(name: &str) -> Result<(), String> {
    if matches!(name, "run_skill" | "save_skill" | "list_skills" | "") {
        return Err("A skill step cannot call the skill tools".to_string());
    }
    let known = tool_definitions()
        .as_array()
        .map(|tools| tools.iter().any(|tool| tool.pointer("/function/name").and_then(|value| value.as_str()) == Some(name)))
        .unwrap_or(false);
    if known {
        Ok(())
    } else {
        Err(format!("Unknown tool in skill: {name}"))
    }
}

fn is_skill_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    first.is_ascii_lowercase()
        && name.len() <= 64
        && name.chars().all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

fn resolve_value(template: &Value, scope: &Value) -> Value {
    match template {
        Value::Object(map) => {
            let mut resolved = serde_json::Map::new();
            for (key, value) in map {
                let next = resolve_value(value, scope);
                if next.is_null() || next.as_str().is_some_and(|text| text.is_empty()) {
                    continue;
                }
                resolved.insert(key.clone(), next);
            }
            Value::Object(resolved)
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| resolve_value(item, scope)).collect()),
        Value::String(text) => resolve_string(text, scope),
        other => other.clone(),
    }
}

fn resolve_string(text: &str, scope: &Value) -> Value {
    if let Some(path) = exact_template(text) {
        return lookup_path(scope, path).unwrap_or(Value::Null);
    }
    if !text.contains("{{") {
        return Value::String(text.to_string());
    }
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            output.push_str(&rest[start..]);
            return Value::String(output);
        };
        let path = after[..end].trim();
        output.push_str(&lookup_path(scope, path).map(|value| match value {
            Value::String(text) => text,
            other => other.to_string(),
        }).unwrap_or_default());
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    Value::String(output)
}

fn exact_template(text: &str) -> Option<&str> {
    let text = text.trim();
    let inner = text.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    if inner.is_empty() || inner.contains("{{") { None } else { Some(inner) }
}

fn lookup_path(scope: &Value, path: &str) -> Option<Value> {
    let mut current = scope;
    for part in path.split('.') {
        current = if let Ok(index) = part.parse::<usize>() {
            current.get(index)?
        } else {
            current.get(part)?
        };
    }
    Some(current.clone())
}

fn skills_dir() -> Result<std::path::PathBuf, String> {
    Ok(crate::t_config::get_app_data_dir()?.join("skills"))
}

fn search_photos(args: &Value) -> Result<Value, String> {
    let limit = args.get("limit").and_then(|value| value.as_i64()).unwrap_or(30).clamp(1, 100);
    let text = args.get("text").and_then(|value| value.as_str()).unwrap_or("").trim().to_string();
    if needs_smart_search(args) {
        let rules = smart_rules_from_search(args)?;
        let params = smart_params(&rules, "all")?;
        let ids = AFile::get_smart_query_file_ids(&params)?;
        return Ok(photo_page(&ids, limit, &text));
    }
    let mut params = empty_query();
    params.search_file_name = text.clone();
    params.is_favorite = args.get("favorite").and_then(|value| value.as_bool()).unwrap_or(false);
    params.rating = args.get("rating").and_then(|value| value.as_i64()).unwrap_or(-1);
    params.make = args.get("make").and_then(|value| value.as_str()).unwrap_or("").to_string();
    params.model = args.get("model").and_then(|value| value.as_str()).unwrap_or("").to_string();
    params.search_file_type = match args.get("fileType").and_then(|value| value.as_str()).unwrap_or("") {
        "image" => 1,
        "video" => 2,
        "audio" => 4,
        _ => 0,
    };
    params.sort_type = 0;
    params.sort_order = 1;
    let ids = AFile::get_query_file_ids(&params)?;
    Ok(photo_page(&ids, limit, &text))
}

fn needs_smart_search(args: &Value) -> bool {
    args.get("ratingMin").and_then(|value| value.as_i64()).is_some()
        || args.get("takenAfter").and_then(|value| value.as_str()).is_some()
        || args.get("takenBefore").and_then(|value| value.as_str()).is_some()
        || args.get("tag").and_then(|value| value.as_str()).is_some_and(|value| !value.trim().is_empty())
        || args.get("person").and_then(|value| value.as_str()).is_some_and(|value| !value.trim().is_empty())
}

fn smart_rules_from_search(args: &Value) -> Result<Vec<Value>, String> {
    let mut rules = Vec::new();
    if let Some(text) = args.get("text").and_then(|value| value.as_str()).map(str::trim).filter(|value| !value.is_empty()) {
        rules.push(json!({"field":"name","operator":"contains","value": text}));
    }
    if args.get("favorite").and_then(|value| value.as_bool()) == Some(true) {
        rules.push(json!({"field":"favorite","operator":"is","value": true}));
    }
    if let Some(min) = args.get("ratingMin").and_then(|value| value.as_i64()) {
        rules.push(json!({"field":"rating","operator":"gte","value": min}));
    } else if let Some(rating) = args.get("rating").and_then(|value| value.as_i64()) {
        rules.push(json!({"field":"rating","operator":"is","value": rating}));
    }
    if let Some(after) = args.get("takenAfter").and_then(|value| value.as_str()) {
        rules.push(json!({"field":"date_taken","operator":"after","value": after}));
    }
    if let Some(before) = args.get("takenBefore").and_then(|value| value.as_str()) {
        rules.push(json!({"field":"date_taken","operator":"before","value": before}));
    }
    if let Some(tag) = args.get("tag").and_then(|value| value.as_str()).map(str::trim).filter(|value| !value.is_empty()) {
        rules.push(json!({"field":"tag","operator":"has","value": tag}));
    }
    if let Some(person) = args.get("person").and_then(|value| value.as_str()).map(str::trim).filter(|value| !value.is_empty()) {
        rules.push(json!({"field":"person","operator":"has","value": person}));
    }
    if let Some(kind) = args.get("fileType").and_then(|value| value.as_str()) {
        let mask = match kind { "image" => 1, "video" => 2, "audio" => 4, _ => 0 };
        if mask > 0 {
            rules.push(json!({"field":"file_type","operator":"is","value": mask}));
        }
    }
    Ok(rules)
}

fn photo_page(ids: &[i64], limit: i64, text: &str) -> Value {
    let shown = ids.iter().take(limit as usize).copied().collect::<Vec<_>>();
    let files = summarize_ids(&shown).unwrap_or_default();
    json!({
        "total": ids.len(),
        "files": files,
        "_action": { "type": "show_files", "title": text, "fileIds": shown }
    })
}

fn summarize_ids(ids: &[i64]) -> Result<Vec<Value>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = crate::t_sqlite::open_conn()?;
    let mut stmt = conn
        .prepare(
            "SELECT a.id, a.name, b.path, IFNULL(a.rating, 0), IFNULL(a.e_title, ''), IFNULL(a.taken_date, 0)
             FROM afiles a JOIN afolders b ON b.id = a.folder_id WHERE a.id = ?1",
        )
        .map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    for id in ids {
        if let Ok((file_id, name, folder, rating, title, taken)) = stmt.query_row(rusqlite::params![id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        }) {
            files.push(json!({
                "fileId": file_id,
                "name": name,
                "path": crate::t_utils::get_file_path(&folder, &name),
                "rating": rating,
                "title": title,
                "taken": taken,
            }));
        }
    }
    Ok(files)
}

fn create_smart_album(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let name = required_str(args, "name")?;
    let match_mode = if args.get("match").and_then(|value| value.as_str()) == Some("any") { "any" } else { "all" };
    let raw_rules = args.get("rules").and_then(|value| value.as_array()).ok_or("Rules are required")?;
    let rules = normalize_rules(raw_rules)?;
    if rules.is_empty() {
        return Err("Add at least one rule".to_string());
    }
    let params = smart_params(&rules, match_mode)?;
    let (count, _) = AFile::get_smart_query_count_and_sum(&params)?;
    let now = chrono::Utc::now().timestamp();
    let album = CustomSmartAlbumState {
        id: Uuid::new_v4().to_string(),
        name: name.clone(),
        description: String::new(),
        source: "rules".to_string(),
        query: SmartAlbumQueryState {
            version: 1,
            r#match: match_mode.to_string(),
            rules: rules
                .iter()
                .map(|rule| SmartAlbumRuleState {
                    id: Uuid::new_v4().to_string(),
                    field: rule["field"].as_str().unwrap_or_default().to_string(),
                    operator: rule["operator"].as_str().unwrap_or_default().to_string(),
                    value: rule["value"].clone(),
                })
                .collect(),
        },
        sort: SmartAlbumSortState::default(),
        group: SmartAlbumGroupState::default(),
        cover_file_id: None,
        count: Some(count),
        created_at: now,
        updated_at: now,
    };
    let mut config = load_app_config()?;
    let library = config
        .libraries
        .iter_mut()
        .find(|library| library.id == config.current_library_id)
        .ok_or("Library not found")?;
    library.state.smart_albums.push(album.clone());
    save_app_config(&config)?;
    let payload = serde_json::to_value(&album).map_err(|error| error.to_string())?;
    let _ = app.emit("agent-smart-album", &payload);
    Ok(json!({
        "smartAlbum": payload,
        "count": count,
        "_action": { "type": "open_smart_album", "id": album.id, "name": album.name }
    }))
}

fn smart_params(rules: &[Value], match_mode: &str) -> Result<SmartQueryParams, String> {
    serde_json::from_value(json!({
        "version": 1,
        "match": match_mode,
        "rules": rules,
        "sortType": 0,
        "sortOrder": 1,
    }))
    .map_err(|error| error.to_string())
}

fn normalize_rules(rules: &[Value]) -> Result<Vec<Value>, String> {
    let mut normalized = Vec::new();
    for rule in rules.iter().take(12) {
        let field = rule.get("field").and_then(|value| value.as_str()).unwrap_or("").trim().to_string();
        let operator = rule.get("operator").and_then(|value| value.as_str()).unwrap_or("is").trim().to_string();
        if !matches!(
            field.as_str(),
            "name" | "favorite" | "rating" | "culling" | "tag" | "person" | "date_taken" | "date_created"
                | "date_modified" | "file_type" | "extension" | "album" | "collection" | "camera" | "lens"
                | "location" | "has_gps" | "orientation" | "width" | "height" | "size"
        ) {
            return Err(format!("Unsupported filter field: {field}"));
        }
        let mut value = rule.get("value").cloned().unwrap_or(Value::Null);
        if matches!(field.as_str(), "date_taken" | "date_created" | "date_modified") {
            value = normalize_date_value(&operator, value)?;
        }
        if field == "tag" {
            if let Some(name) = value.as_str() {
                value = json!(tag_id_by_name(name)?);
            }
        }
        if field == "person" {
            if let Some(name) = value.as_str() {
                value = json!(person_id_by_name(name)?);
            }
        }
        normalized.push(json!({
            "id": Uuid::new_v4().to_string(),
            "field": field,
            "operator": operator,
            "value": value,
        }));
    }
    Ok(normalized)
}

fn normalize_date_value(operator: &str, value: Value) -> Result<Value, String> {
    match operator {
        "after" | "before" => {
            let text = value.as_str().ok_or("Use a YYYY-MM-DD date")?;
            let unix = date_to_unix(text).ok_or("Use a YYYY-MM-DD date")?;
            Ok(json!({ "value": unix }))
        }
        "between" => {
            let start = value.get("start").and_then(|item| item.as_str()).ok_or("Date start is required")?;
            let end = value.get("end").and_then(|item| item.as_str()).ok_or("Date end is required")?;
            Ok(json!({ "start": date_to_unix(start).ok_or("Invalid start date")?, "end": date_to_unix(end).ok_or("Invalid end date")? }))
        }
        "in_last" | "older_than" => Ok(value),
        "is" => Ok(value),
        _ => Err(format!("Unsupported date operator: {operator}")),
    }
}

fn date_to_unix(value: &str) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()?;
    let naive = date.and_hms_opt(0, 0, 0)?;
    chrono::Local.from_local_datetime(&naive).single().map(|time| time.timestamp())
}

fn import_files(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let destination = required_str(args, "destinationPath")?;
    let sources = args
        .get("sourcePaths")
        .and_then(|value| value.as_array())
        .ok_or("sourcePaths is required")?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .take(40)
        .collect::<Vec<_>>();
    if sources.is_empty() {
        return Err("Choose files to import".to_string());
    }
    let conn = crate::t_sqlite::open_conn()?;
    let (folder_id, folder_path, album_id): (i64, String, i64) = conn
        .query_row(
            "SELECT id, path, album_id FROM afolders WHERE path = ?1",
            rusqlite::params![destination],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| format!("Destination is not a folder in the library: {destination}"))?;
    let mut imported = Vec::new();
    let mut errors = Vec::new();
    for source in sources {
        match crate::t_cmds::import_file(&source, folder_id, &folder_path) {
            Ok(Some(file)) => imported.push(file.id.unwrap_or(0)),
            Ok(None) => errors.push(format!("{source}: skipped")),
            Err(error) => errors.push(format!("{source}: {error}")),
        }
    }
    let _ = app.emit("import-files-added", json!({ "albumId": album_id }));
    Ok(json!({ "albumId": album_id, "imported": imported, "errors": errors }))
}

fn start_index(app: &AppHandle, album_id: i64) -> Result<(), String> {
    let state = app.state::<crate::t_cmds::IndexCancellation>();
    state.0.lock().map_err(|_| "Index state is unavailable".to_string())?.insert(album_id, false);
    let token = state.0.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::t_utils::index_album_worker(&app, token, album_id, 512, false, None, false).await {
            eprintln!("Agent index failed for album {album_id}: {error}");
        }
    });
    Ok(())
}

fn list_people() -> Result<Value, String> {
    let conn = crate::t_sqlite::open_conn()?;
    let mut stmt = conn
        .prepare("SELECT id, COALESCE(name, '') FROM persons ORDER BY name COLLATE NOCASE LIMIT 200")
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok(json!({ "id": row.get::<_, i64>(0)?, "name": row.get::<_, String>(1)? })))
        .map_err(|error| error.to_string())?;
    let people = rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
    Ok(json!({ "people": people }))
}

fn ensure_tag(name: &str) -> Result<i64, String> {
    let conn = crate::t_sqlite::open_conn()?;
    if let Ok(id) = conn.query_row(
        "SELECT id FROM atags WHERE name = ?1 COLLATE NOCASE",
        rusqlite::params![name],
        |row| row.get::<_, i64>(0),
    ) {
        return Ok(id);
    }
    Ok(ATag::add(name, None)?.id)
}

fn tag_id_by_name(name: &str) -> Result<i64, String> {
    ensure_tag(name)
}

fn person_id_by_name(name: &str) -> Result<i64, String> {
    let conn = crate::t_sqlite::open_conn()?;
    conn.query_row(
        "SELECT id FROM persons WHERE name = ?1 COLLATE NOCASE",
        rusqlite::params![name],
        |row| row.get(0),
    )
    .map_err(|_| format!("No person named {name}"))
}

fn collection_id(args: &Value) -> Result<i64, String> {
    if let Some(id) = args.get("collectionId").and_then(|value| value.as_i64()) {
        return Ok(id);
    }
    let name = required_str(args, "name")?;
    ACollection::list()?
        .into_iter()
        .find(|collection| collection.name.eq_ignore_ascii_case(&name))
        .map(|collection| collection.id)
        .ok_or_else(|| format!("No collection named {name}"))
}

fn required_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

fn required_ids(args: &Value, key: &str) -> Result<Vec<i64>, String> {
    let ids = args
        .get(key)
        .and_then(|value| value.as_array())
        .ok_or_else(|| format!("{key} is required"))?
        .iter()
        .filter_map(|value| value.as_i64())
        .filter(|id| *id > 0)
        .take(200)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err(format!("{key} is required"));
    }
    Ok(ids)
}

fn empty_query() -> QueryParams {
    serde_json::from_value(json!({
        "searchFileName": "", "searchFileType": 0, "sortType": 0, "sortOrder": 1,
        "searchAllSubfolders": "", "searchFolder": "", "startDate": 0, "endDate": 0,
        "calendarSort": 0, "make": "", "model": "", "lensMake": "", "lensModel": "",
        "locationAdmin1": "", "locationName": "", "isFavorite": false, "rating": -1,
        "tagId": 0, "personId": 0
    }))
    .unwrap_or_else(|_| panic!("query defaults"))
}

fn settings_view(settings: &AgentSettings) -> Value {
    let hint = if settings.api_key.len() >= 4 {
        format!("••••{}", &settings.api_key[settings.api_key.len() - 4..])
    } else if settings.api_key.is_empty() {
        String::new()
    } else {
        "••••".to_string()
    };
    json!({
        "provider": settings.provider,
        "baseUrl": settings.base_url,
        "model": settings.model,
        "hasApiKey": !settings.api_key.is_empty(),
        "apiKeyHint": hint,
        "mcpEnabled": settings.mcp_enabled,
        "mcpPort": settings.mcp_port,
        "mcpToken": settings.mcp_token,
        "mcpUrl": format!("http://127.0.0.1:{}/mcp", settings.mcp_port),
        "mcpRunning": MCP_RUNNING.load(Ordering::Relaxed),
        "mcpError": MCP_ERROR.lock().map(|error| error.clone()).unwrap_or_default(),
    })
}

fn settings_path() -> Result<std::path::PathBuf, String> {
    let dir = crate::t_config::get_app_data_dir()?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join("agent.json"))
}

fn load_settings() -> Result<AgentSettings, String> {
    let path = settings_path()?;
    if !path.exists() {
        let settings = AgentSettings::default();
        save_settings(&settings)?;
        return Ok(settings);
    }
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let mut settings: AgentSettings = serde_json::from_str(&text).unwrap_or_default();
    if settings.mcp_token.trim().is_empty() {
        settings.mcp_token = Uuid::new_v4().to_string();
        save_settings(&settings)?;
    }
    if settings.mcp_port == 0 {
        settings.mcp_port = DEFAULT_PORT;
    }
    Ok(settings)
}

fn save_settings(settings: &AgentSettings) -> Result<(), String> {
    let path = settings_path()?;
    let text = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, text).map_err(|error| error.to_string())
}

fn preset_base_url(provider: &str) -> &'static str {
    match provider {
        "openrouter" => "https://openrouter.ai/api/v1",
        "ollama" => "http://127.0.0.1:11434/v1",
        "openai" => "https://api.openai.com/v1",
        _ => "",
    }
}

fn normalize_base_url(value: &str) -> String {
    let mut url = value.trim().trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/models"] {
        if let Some(stripped) = url.strip_suffix(suffix) {
            url = stripped.trim_end_matches('/').to_string();
        }
    }
    url
}

fn trim_error(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(280).collect()
}

fn missing_tool_endpoint(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("support tool") || error.contains("tool-endpoint-missing")
}

fn model_supports_tools(row: &Value) -> bool {
    row.get("supported_parameters")
        .and_then(|value| value.as_array())
        .is_some_and(|parameters| parameters.iter().any(|parameter| parameter.as_str() == Some("tools")))
}

fn text_tool_instructions() -> String {
    let mut lines = vec![
        "Native tool calling is not available for this model. When you need a Lap tool, reply with only a JSON object and no other text: {\"tool\":\"tool_name\",\"arguments\":{}}."
            .to_string(),
        "When you can answer without a tool, reply in plain text.".to_string(),
        "Tools:".to_string(),
    ];
    if let Some(tools) = tool_definitions().as_array() {
        for tool in tools {
            let name = tool.pointer("/function/name").and_then(|value| value.as_str()).unwrap_or("");
            let description = tool.pointer("/function/description").and_then(|value| value.as_str()).unwrap_or("");
            lines.push(format!("- {name}: {description}"));
        }
    }
    lines.join("\n")
}

fn parse_text_tool(content: &str) -> Option<(String, Value)> {
    let trimmed = strip_fence(content.trim());
    if let Some(parsed) = parse_tool_value(trimmed.trim()) {
        return Some(parsed);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start || trimmed[..start].trim().chars().count() > 40 {
        return None;
    }
    parse_tool_value(&trimmed[start..=end])
}

fn strip_fence(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("```") else {
        return content;
    };
    let rest = rest.trim_start_matches(|character: char| character.is_ascii_alphanumeric());
    let rest = rest.trim_start();
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

fn parse_tool_value(text: &str) -> Option<(String, Value)> {
    let value: Value = serde_json::from_str(text).ok()?;
    let name = value.get("tool").or_else(|| value.get("name"))?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let arguments = value
        .get("arguments")
        .or_else(|| value.get("parameters"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    Some((name.to_string(), arguments))
}

pub fn restart_mcp() {
    let settings = match load_settings() {
        Ok(settings) => settings,
        Err(error) => {
            set_mcp_error(error);
            return;
        }
    };
    if let Ok(mut server) = SERVER.lock() {
        if let Some(handle) = server.take() {
            handle.abort();
        }
        MCP_RUNNING.store(false, Ordering::Relaxed);
        if !settings.mcp_enabled {
            set_mcp_error(String::new());
            return;
        }
        let port = settings.mcp_port;
        let token = settings.mcp_token.clone();
        *server = Some(tauri::async_runtime::spawn(async move {
            if let Err(error) = serve(port, token).await {
                MCP_RUNNING.store(false, Ordering::Relaxed);
                set_mcp_error(error);
            }
        }));
    }
}

fn set_mcp_error(error: String) {
    if let Ok(mut slot) = MCP_ERROR.lock() {
        *slot = error;
    }
}

async fn serve(port: u16, token: String) -> Result<(), String> {
    let listener = bind_with_retry(port).await?;
    MCP_RUNNING.store(true, Ordering::Relaxed);
    set_mcp_error(String::new());
    loop {
        let (stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
        let token = token.clone();
        tokio::spawn(async move {
            let _ = handle_connection(stream, &token).await;
        });
    }
}

async fn bind_with_retry(port: u16) -> Result<TcpListener, String> {
    let mut last = String::new();
    for _ in 0..20 {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) => {
                last = error.to_string();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    Err(format!("Could not listen on 127.0.0.1:{port}: {last}"))
}

async fn handle_connection(mut stream: TcpStream, token: &str) -> Result<(), ()> {
    let mut buffer = vec![0u8; 8192];
    let mut received = 0usize;
    loop {
        if received >= MAX_BODY {
            break;
        }
        let read = stream.read(&mut buffer[received..]).await.map_err(|_| ())?;
        if read == 0 {
            break;
        }
        received += read;
        if request_complete(&buffer[..received]) {
            break;
        }
    }
    let request = String::from_utf8_lossy(&buffer[..received]);
    let (method, path, headers, body) = split_http(&request);
    if method == "OPTIONS" {
        let _ = write_http(&mut stream, 204, "text/plain", &headers_cors(), "").await;
        return Ok(());
    }
    if path != "/mcp" && path != "/mcp/" {
        let _ = write_http(&mut stream, 404, "application/json", &headers_cors(), "{\"error\":\"not found\"}").await;
        return Ok(());
    }
    if !authorized(&headers, token) {
        let _ = write_http(&mut stream, 401, "application/json", &headers_cors(), "{\"error\":\"unauthorized\"}").await;
        return Ok(());
    }
    if method == "GET" {
        let info = json!({"name":"lap","transport":"streamable-http","url":"http://127.0.0.1/mcp"}).to_string();
        let _ = write_http(&mut stream, 200, "application/json", &headers_cors(), &info).await;
        return Ok(());
    }
    if method != "POST" {
        let _ = write_http(&mut stream, 405, "application/json", &headers_cors(), "{\"error\":\"method\"}").await;
        return Ok(());
    }
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let Some(response) = rpc_response(&parsed) else {
        let _ = write_http(&mut stream, 202, "application/json", &headers_cors(), "").await;
        return Ok(());
    };
    let _ = write_http(&mut stream, 200, "application/json", &headers_cors(), &response.to_string()).await;
    Ok(())
}

fn rpc_response(request: &Value) -> Option<Value> {
    if request.get("id").is_none() {
        return None;
    }
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(|value| value.as_str()).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "lap", "version": "0.3.2" }
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": mcp_tools() }),
        "tools/call" => match mcp_call(&params) {
            Ok(value) => value,
            Err(error) => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": [{ "type": "text", "text": error }], "isError": true }
                }));
            }
        },
        "resources/list" => json!({ "resources": [] }),
        "prompts/list" => json!({ "prompts": [] }),
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn mcp_tools() -> Value {
    tool_definitions()
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let function = tool.get("function")?;
                    Some(json!({
                        "name": function.get("name")?,
                        "description": function.get("description")?,
                        "inputSchema": function.get("parameters")?,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .map(Value::Array)
        .unwrap_or_else(|| json!([]))
}

fn mcp_call(params: &Value) -> Result<Value, String> {
    let name = params.get("name").and_then(|value| value.as_str()).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let app = APP.get().ok_or("Lap is still starting")?;
    let value = dispatch_tool(app, name, &arguments)?;
    let text = value.to_string();
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
}

fn authorized(headers: &[(String, String)], token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    headers.iter().any(|(name, value)| {
        if !name.eq_ignore_ascii_case("authorization") {
            return false;
        }
        let Some(presented) = value.trim().strip_prefix("Bearer ") else {
            return false;
        };
        presented.trim() == token
    })
}

fn request_complete(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let Some((head, _)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let header_len = head.len() + 4;
    bytes.len() >= header_len + length
}

fn split_http(request: &str) -> (String, String, Vec<(String, String)>, &str) {
    let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    (method, path, headers, body)
}

fn headers_cors() -> Vec<(&'static str, String)> {
    vec![
        ("Access-Control-Allow-Origin", "*".to_string()),
        ("Access-Control-Allow-Headers", "Authorization, Content-Type, Accept, Mcp-Session-Id".to_string()),
        ("Access-Control-Allow-Methods", "POST, GET, OPTIONS, DELETE".to_string()),
    ]
}

async fn write_http(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    extra: &[(&str, String)],
    body: &str,
) -> Result<(), ()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.map_err(|_| ())?;
    if status != 204 {
        stream.write_all(body.as_bytes()).await.map_err(|_| ())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_base_url_strips_completion_suffix() {
        assert_eq!(normalize_base_url("https://api.openai.com/v1/chat/completions/"), "https://api.openai.com/v1");
    }

    #[test]
    fn agent_rpc_lists_metadata_tools_and_rejects_unknown_methods() {
        let listed = rpc_response(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        let names = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"update_metadata"));
        assert!(names.contains(&"search_metadata"));
        assert!(names.contains(&"create_smart_album"));
        let missing = rpc_response(&json!({"jsonrpc":"2.0","id":2,"method":"nope"})).unwrap();
        assert_eq!(missing["error"]["code"], -32601);
        assert!(rpc_response(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
    }

    #[test]
    fn agent_skill_templates_pass_tool_arguments_without_extra_text() {
        let scope = json!({
            "keyword": "Monic",
            "minutes": 5,
            "found": { "fileIds": [4, 9], "fileCount": 2 }
        });
        let resolved = resolve_value(&json!({
            "keyword": "{{keyword}}",
            "minutes": "{{minutes}}",
            "camera": "{{camera}}",
            "fileIds": "{{found.fileIds}}"
        }), &scope);
        assert_eq!(resolved["keyword"], "Monic");
        assert_eq!(resolved["minutes"], 5);
        assert!(resolved.get("camera").is_none());
        assert_eq!(resolved["fileIds"], json!([4, 9]));
        let skill = builtin_skills().into_iter().find(|skill| skill["name"] == "around-keyword").unwrap();
        assert_eq!(skill["steps"][0]["tool"], "photos_around");
        assert!(ensure_skill_tool("run_skill").is_err());
        assert!(skill_catalog().contains("around-keyword(keyword, minutes?)"));
        assert!(skill_catalog().contains("trash-rejected()"));
    }

    #[test]
    fn agent_photos_around_keyword_uses_capture_time() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE afolders(id INTEGER PRIMARY KEY, path TEXT, album_id INTEGER);
             INSERT INTO afolders VALUES(1, '/photos', 1);
             CREATE TABLE afiles(id INTEGER PRIMARY KEY, folder_id INTEGER, name TEXT, taken_date INTEGER,
               e_keywords TEXT, e_make TEXT, e_model TEXT, e_lens_make TEXT, e_lens_model TEXT,
               e_title TEXT, e_description TEXT, comments TEXT, e_location TEXT, geo_name TEXT,
               geo_admin1 TEXT, geo_cc TEXT, rating INTEGER, is_favorite INTEGER, file_type INTEGER,
               live_photo_video_id INTEGER);
             INSERT INTO afiles VALUES
               (1, 1, 'monic.jpg', 1700000000, '[\"Monic\"]', '', '', '', '', '', '', '', '', '', '', '', 0, 0, 1, NULL),
               (2, 1, 'near.jpg', 1700000180, '[]', '', '', '', '', '', '', '', '', '', '', '', 0, 0, 1, NULL),
               (3, 1, 'later.jpg', 1700004000, '[]', '', '', '', '', '', '', '', '', '', '', '', 0, 0, 1, NULL);
             CREATE TABLE atags(id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO atags VALUES(1, 'Monic');
             CREATE TABLE afile_tags(file_id INTEGER, tag_id INTEGER);
             INSERT INTO afile_tags VALUES(1, 1);",
        )
        .unwrap();
        let around = photos_around_on(&conn, 0, "monic", 300, 20).unwrap();
        let ids = around.iter().filter_map(|file| file["fileId"].as_i64()).collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 2]);
        let cameras = query_photos_on(
            &conn,
            &PhotoFilter {
                keyword: "monic".to_string(),
                camera: String::new(),
                lens: String::new(),
                place: String::new(),
                person: String::new(),
                text: String::new(),
                favorite: None,
                rating_min: None,
                culling: None,
                file_type: 0,
                album_id: None,
                collection_id: None,
                taken_from: None,
                taken_to: None,
                limit: 10,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(cameras[0]["fileId"], 1);
        conn.execute_batch(
            "ALTER TABLE afiles ADD COLUMN culling_flag INTEGER;
             UPDATE afiles SET culling_flag = 1 WHERE id = 1;
             UPDATE afiles SET culling_flag = 2 WHERE id = 2;",
        )
        .unwrap();
        let mut picked = PhotoFilter {
            keyword: String::new(),
            camera: String::new(),
            lens: String::new(),
            place: String::new(),
            person: String::new(),
            text: String::new(),
            favorite: None,
            rating_min: None,
            culling: Some(1),
            file_type: 0,
            album_id: None,
            collection_id: None,
            taken_from: None,
            taken_to: None,
            limit: 10,
            offset: 0,
        };
        assert_eq!(query_photos_on(&conn, &picked).unwrap()[0]["fileId"], 1);
        picked.culling = Some(2);
        assert_eq!(query_photos_on(&conn, &picked).unwrap()[0]["fileId"], 2);
        assert!(parse_culling(&json!("unreviewed")) == Some(0));
        assert!(parse_culling(&json!("picked")) == Some(1));
        assert!(parse_culling(&json!("rejected")) == Some(2));
        assert!(cameras[0]["taken"].as_str().unwrap().contains(':'));
    }

    #[test]
    fn agent_openrouter_tool_error_uses_text_tools() {
        let error = r#"{"error":{"message":"No endpoints found that support tool use. Try disabling \"list_albums\"."}}"#;
        assert!(missing_tool_endpoint(error));
        assert!(!missing_tool_endpoint("hello"));
        assert_eq!(
            parse_text_tool(r#"{"tool":"list_albums","arguments":{}}"#).map(|(name, _)| name),
            Some("list_albums".to_string())
        );
        assert!(parse_text_tool("hello").is_none());
        let fenced = "```json\n{\"tool\":\"search_metadata\",\"arguments\":{\"place\":\"Oslo\"}}\n```";
        let (name, args) = parse_text_tool(fenced).unwrap();
        assert_eq!(name, "search_metadata");
        assert_eq!(args["place"], "Oslo");
    }

    #[test]
    fn agent_http_auth_requires_the_bearer_token() {
        assert!(!authorized(&[("Authorization".to_string(), "Bearer wrong".to_string())], "secret"));
        assert!(authorized(&[("authorization".to_string(), "Bearer secret".to_string())], "secret"));
        let (method, path, headers, body) = split_http("POST /mcp?x=1 HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 2\r\n\r\n{}");
        assert_eq!(method, "POST");
        assert_eq!(path, "/mcp");
        assert_eq!(body, "{}");
        assert!(authorized(&headers, "secret"));
    }
}
