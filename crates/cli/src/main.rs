//! tauri-connector CLI — interact with Tauri apps from the terminal.

use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use connector_client::ConnectorClient;

mod commands;
mod hook;
mod snapshot;
mod update;

use snapshot::RefMap;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 9555;

fn ref_cache_path() -> PathBuf {
    std::env::temp_dir().join("tauri-connector-refs.json")
}

fn load_refs() -> RefMap {
    fs::read_to_string(ref_cache_path())
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

fn save_refs(refs: &RefMap) {
    if let Ok(json) = serde_json::to_string(refs) {
        let _ = fs::write(ref_cache_path(), json);
    }
}

#[derive(Parser)]
#[command(
    name = "tauri-connector",
    about = "CLI for interacting with Tauri apps",
    version = env!("CARGO_PKG_VERSION"),
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Take DOM snapshot with ref IDs
    Snapshot {
        /// Interactive only (elements with refs)
        #[arg(short, long)]
        interactive: bool,
        /// Compact (remove structural wrappers)
        #[arg(short, long)]
        compact: bool,
        /// Max depth (0 = unlimited)
        #[arg(short, long, default_value_t = 0)]
        depth: usize,
        /// Max elements (0 = unlimited)
        #[arg(long, default_value_t = 0)]
        max_elements: usize,
        /// Scope to CSS selector
        #[arg(short, long)]
        selector: Option<String>,
        /// Snapshot mode: ai, accessibility, structure
        #[arg(long)]
        mode: Option<String>,
        /// Disable React component name enrichment
        #[arg(long)]
        no_react: bool,
        /// Disable portal stitching
        #[arg(long)]
        no_portals: bool,
    },
    /// Click an element
    Click {
        /// @ref or CSS selector
        target: String,
    },
    /// Double-click an element
    Dblclick {
        target: String,
    },
    /// Hover over element
    Hover {
        target: String,
    },
    /// Drag element to target
    Drag {
        /// Source: @ref or CSS selector
        source: String,
        /// Target: @ref, CSS selector, or "x,y" coordinates
        target: String,
        /// Number of intermediate move events (default 10)
        #[arg(short, long, default_value_t = 10)]
        steps: u32,
        /// Total drag duration in ms (default 300)
        #[arg(short, long, default_value_t = 300)]
        duration: u32,
        /// Drag strategy: auto, pointer, html5dnd (default auto)
        #[arg(long, default_value = "auto")]
        strategy: String,
    },
    /// Focus an element
    Focus {
        target: String,
    },
    /// Clear and fill input
    Fill {
        target: String,
        text: Vec<String>,
    },
    /// Type text character by character
    Type {
        target: String,
        text: Vec<String>,
    },
    /// Check checkbox
    Check {
        target: String,
    },
    /// Uncheck checkbox
    Uncheck {
        target: String,
    },
    /// Select option(s) in a <select>
    Select {
        target: String,
        values: Vec<String>,
    },
    /// Scroll page or element
    Scroll {
        /// Direction: up, down, left, right
        #[arg(default_value = "down")]
        direction: String,
        /// Scroll amount in pixels
        #[arg(default_value_t = 300)]
        amount: i32,
        /// Target element selector
        #[arg(long)]
        selector: Option<String>,
    },
    /// Scroll element into view
    Scrollintoview {
        target: String,
    },
    /// Press a key (Enter, Tab, Escape, etc.)
    Press {
        key: String,
    },
    /// Get property from element or page
    Get {
        /// Property: title, url, text, html, value, attr, box, styles, count
        prop: String,
        /// @ref or CSS selector (not needed for title/url)
        target: Option<String>,
        /// Attribute name (for 'attr' prop)
        extra: Option<String>,
    },
    /// Wait for element or text
    Wait {
        /// CSS selector to wait for
        selector: Option<String>,
        /// Wait for this text to appear
        #[arg(long)]
        text: Option<String>,
        /// Timeout in milliseconds
        #[arg(long, default_value_t = 5000)]
        timeout: u64,
    },
    /// Execute JavaScript
    Eval {
        /// JS expression
        script: Vec<String>,
    },
    /// Console logs
    Logs {
        /// Number of lines
        #[arg(short = 'n', long, default_value_t = 20)]
        lines: usize,
        /// Filter string
        #[arg(short, long)]
        filter: Option<String>,
        /// Log level filter (e.g. error, warn, info)
        #[arg(short, long)]
        level: Option<String>,
        /// Regex pattern to match
        #[arg(short, long)]
        pattern: Option<String>,
    },
    /// Take a screenshot and save to file
    Screenshot {
        /// Output file path (e.g. /tmp/shot.png)
        output: String,
        /// Image format: png, jpeg, webp
        #[arg(short, long, default_value = "png")]
        format: String,
        /// JPEG/WebP quality (0-100)
        #[arg(short, long, default_value_t = 80)]
        quality: u8,
        /// Max width in pixels (resize if larger)
        #[arg(short, long)]
        max_width: Option<u32>,
    },
    /// Get cached DOM snapshot (pushed from frontend)
    Dom {
        /// Window ID
        #[arg(long, default_value = "main")]
        window_id: String,
    },
    /// Find elements by CSS selector, XPath, or text
    Find {
        /// Selector or text to search for
        selector: String,
        /// Search strategy: css, xpath, text
        #[arg(short, long, default_value = "css")]
        strategy: String,
    },
    /// Get element metadata from Alt+Shift+Click picker
    Pointed,
    /// Resize a window
    Resize {
        /// Width in pixels
        width: u32,
        /// Height in pixels
        height: u32,
        /// Window ID
        #[arg(long, default_value = "main")]
        window_id: String,
    },
    /// Execute a Tauri IPC command via invoke()
    Ipc {
        #[command(subcommand)]
        action: IpcCommands,
    },
    /// Emit a custom Tauri event
    Emit {
        /// Event name
        event: String,
        /// JSON payload
        #[arg(short, long)]
        payload: Option<String>,
    },
    /// Listen for and retrieve Tauri events
    Events {
        #[command(subcommand)]
        action: EventCommands,
    },
    /// Clear log files
    Clear {
        /// What to clear: logs, ipc, events, all
        target: String,
    },
    /// App backend state
    State,
    /// List windows
    Windows,
    /// Check for updates and self-update
    Update {
        /// Just check, don't install
        #[arg(long)]
        check: bool,
    },
    /// Show detailed help with examples
    Examples,
    /// Manage Claude Code auto-detect hook
    Hook {
        #[command(subcommand)]
        action: HookCommands,
    },
}

#[derive(Subcommand)]
enum HookCommands {
    /// Install auto-detect hook for Claude Code
    Install,
    /// Remove auto-detect hook
    Remove,
}

#[derive(Subcommand)]
enum IpcCommands {
    /// Execute a Tauri IPC command
    Exec {
        /// Command name (e.g. "greet")
        command: String,
        /// JSON args (e.g. '{"name":"world"}')
        #[arg(short, long)]
        args: Option<String>,
    },
    /// Start IPC monitoring
    Monitor,
    /// Stop IPC monitoring
    Unmonitor,
    /// Get captured IPC traffic
    Captured {
        /// Filter by command name
        #[arg(short, long)]
        filter: Option<String>,
        /// Regex pattern to match
        #[arg(short, long)]
        pattern: Option<String>,
        /// Only entries since this timestamp (epoch ms)
        #[arg(long)]
        since: Option<u64>,
        /// Max entries to return
        #[arg(short, long, default_value_t = 100)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum EventCommands {
    /// Start listening for events
    Listen {
        /// Comma-separated event names
        events: String,
    },
    /// Get captured events
    Captured {
        /// Regex pattern to match
        #[arg(short, long)]
        pattern: Option<String>,
        /// Only entries since this timestamp (epoch ms)
        #[arg(long)]
        since: Option<u64>,
        /// Max entries to return
        #[arg(short, long, default_value_t = 100)]
        limit: usize,
    },
    /// Stop listening
    Stop,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if matches!(cli.command, Commands::Examples) {
        print_help();
        return;
    }

    if let Commands::Update { check } = &cli.command {
        if let Err(e) = update::run(*check).await {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    if let Commands::Hook { action } = &cli.command {
        let result = match action {
            HookCommands::Install => hook::install(),
            HookCommands::Remove => hook::remove(),
        };
        if let Err(e) = result {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let host = std::env::var("TAURI_CONNECTOR_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let port: u16 = std::env::var("TAURI_CONNECTOR_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let mut client = ConnectorClient::new();

    // Connect with ping fallback
    if let Err(e) = client.connect(&host, port).await {
        eprintln!("Error: Failed to connect to {host}:{port}: {e}");
        std::process::exit(1);
    }

    let mut refs = load_refs();

    let result = match cli.command {
        Commands::Snapshot {
            interactive,
            compact,
            depth,
            max_elements,
            selector,
            mode,
            no_react,
            no_portals,
        } => {
            match commands::snapshot(
                &client, interactive, compact, depth, max_elements,
                selector, mode, !no_react, !no_portals,
            ).await {
                Ok(new_refs) => {
                    refs = new_refs;
                    save_refs(&refs);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Commands::Click { target } => commands::click(&client, &refs, &target).await,
        Commands::Dblclick { target } => commands::dblclick(&client, &refs, &target).await,
        Commands::Hover { target } => commands::hover(&client, &refs, &target).await,
        Commands::Drag { source, target, steps, duration, strategy } => {
            commands::drag(&client, &refs, &source, &target, steps, duration, &strategy).await
        }
        Commands::Focus { target } => commands::focus(&client, &refs, &target).await,
        Commands::Fill { target, text } => {
            commands::fill(&client, &refs, &target, &text.join(" ")).await
        }
        Commands::Type { target, text } => {
            commands::type_text(&client, &refs, &target, &text.join(" ")).await
        }
        Commands::Check { target } => commands::check(&client, &refs, &target).await,
        Commands::Uncheck { target } => commands::uncheck(&client, &refs, &target).await,
        Commands::Select { target, values } => {
            commands::select(&client, &refs, &target, &values).await
        }
        Commands::Scroll {
            direction,
            amount,
            selector,
        } => commands::scroll(&client, &refs, &direction, amount, selector.as_deref()).await,
        Commands::Scrollintoview { target } => {
            commands::scroll_into_view(&client, &refs, &target).await
        }
        Commands::Press { key } => commands::press(&client, &key).await,
        Commands::Get {
            prop,
            target,
            extra,
        } => {
            commands::get_prop(
                &client,
                &refs,
                &prop,
                target.as_deref(),
                extra.as_deref(),
            )
            .await
        }
        Commands::Wait {
            selector,
            text,
            timeout,
        } => commands::wait(&client, selector.as_deref(), text.as_deref(), timeout).await,
        Commands::Eval { script } => commands::eval_js(&client, &script.join(" ")).await,
        Commands::Logs { lines, filter, level, pattern } => {
            commands::logs(&client, lines, filter.as_deref(), level.as_deref(), pattern.as_deref()).await
        }
        Commands::Screenshot {
            output,
            format,
            quality,
            max_width,
        } => commands::screenshot(&client, &output, &format, quality, max_width).await,
        Commands::Dom { window_id } => commands::cached_dom(&client, &window_id).await,
        Commands::Find {
            selector,
            strategy,
        } => commands::find(&client, &selector, &strategy).await,
        Commands::Pointed => commands::pointed(&client).await,
        Commands::Resize {
            width,
            height,
            window_id,
        } => commands::resize(&client, &window_id, width, height).await,
        Commands::Ipc { action } => match action {
            IpcCommands::Exec { command, args } => {
                commands::ipc_exec(&client, &command, args.as_deref()).await
            }
            IpcCommands::Monitor => commands::ipc_monitor(&client, "start").await,
            IpcCommands::Unmonitor => commands::ipc_monitor(&client, "stop").await,
            IpcCommands::Captured { filter, pattern, since, limit } => {
                commands::ipc_captured(&client, filter.as_deref(), pattern.as_deref(), since, limit).await
            }
        },
        Commands::Emit { event, payload } => {
            commands::ipc_emit(&client, &event, payload.as_deref()).await
        }
        Commands::Events { action } => match action {
            EventCommands::Listen { events } => {
                commands::event_listen(&client, &events).await
            }
            EventCommands::Captured { pattern, since, limit } => {
                commands::event_captured(&client, pattern.as_deref(), since, limit).await
            }
            EventCommands::Stop => {
                commands::event_stop(&client).await
            }
        },
        Commands::Clear { target } => {
            commands::clear_logs(&client, &target).await
        },
        Commands::State => commands::state(&client).await,
        Commands::Windows => commands::windows(&client).await,
        Commands::Update { .. } => unreachable!(),
        Commands::Examples => unreachable!(),
        Commands::Hook { .. } => unreachable!(),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        r##"
tauri-connector CLI - interact with Tauri apps

USAGE:
  tauri-connector <command> [args...]

CONNECTION:
  Connects to tauri-plugin-connector WebSocket on
  $TAURI_CONNECTOR_HOST:$TAURI_CONNECTOR_PORT (default: 127.0.0.1:9555)

SNAPSHOT:
  snapshot [-i] [-c] [-d N] [-s "#selector"]
    Take DOM snapshot with ref IDs. Flags:
      -i  Interactive only (elements with refs)
      -c  Compact (remove structural wrappers)
      -d  Max depth
      -s  Scope to CSS selector

INTERACTIONS (use @eN refs from snapshot):
  click <@ref|selector>              Click an element
  dblclick <@ref|selector>           Double-click
  hover <@ref|selector>              Hover over element
  drag <source> <target> [opts]      Drag element to target
  focus <@ref|selector>              Focus an element
  fill <@ref|selector> <text>        Clear and fill input
  type <@ref|selector> <text>        Type text character by character
  check <@ref|selector>              Check checkbox
  uncheck <@ref|selector>            Uncheck checkbox
  select <@ref|selector> <val...>    Select option(s)
  scrollintoview <@ref|selector>     Scroll element into view

KEYBOARD:
  press <key>                        Press a key (Enter, Tab, Escape, etc.)

SCROLL:
  scroll [up|down|left|right] [amount] [--selector <sel>]

GETTERS:
  get title                          Page title
  get url                            Current URL
  get text <@ref|selector>           Text content
  get html <@ref|selector>           Inner HTML
  get value <@ref|selector>          Input value
  get attr <@ref|selector> <name>    Attribute value
  get box <@ref|selector>            Bounding box
  get styles <@ref|selector>         Computed styles
  get count <selector>               Element count

WAIT:
  wait <selector>                    Wait for element
  wait --timeout <ms>                Wait for duration
  wait --text "Success"              Wait for text

SCREENSHOT:
  screenshot <path> [-f png|jpeg|webp] [-q 80] [-m 1280]

DOM:
  dom                                Cached DOM (pushed from frontend)

FIND:
  find <selector> [-s css|xpath|text] Find elements

WINDOW:
  windows                            List windows
  resize <width> <height>            Resize window
  pointed                            Alt+Shift+Click element info

IPC:
  ipc exec <command> [-a '{{"k":"v"}}'] Execute Tauri IPC command
  ipc monitor                         Start IPC monitoring
  ipc unmonitor                       Stop IPC monitoring
  ipc captured [-f filter] [-l 100]   Get captured IPC traffic
  emit <event> [-p '{{"k":"v"}}']       Emit custom Tauri event

OTHER:
  eval <js-expression>               Execute JavaScript
  logs [-n 20] [-f "filter"]         Console logs
  state                              App metadata
  help                               This help

EXAMPLES:
  tauri-connector snapshot -i -c
  tauri-connector click @e7
  tauri-connector fill @e5 "user@example.com"
  tauri-connector screenshot /tmp/shot.png -m 1280
  tauri-connector drag @e3 @e7 --steps 15 --duration 500
  tauri-connector drag "#item" ".drop-zone" --strategy pointer
  tauri-connector find "Submit" -s text
  tauri-connector ipc exec greet -a '{{"name":"world"}}'
  tauri-connector emit my-event -p '{{"foo":42}}'
  tauri-connector resize 1024 768
  tauri-connector eval "document.title"
"##
    );
}
