//! tauri-connector CLI — interact with Tauri apps from the terminal.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use connector_client::discovery::{self, ConnectionOptions};
use connector_client::ConnectorClient;

mod commands;
mod doctor;
mod hook;
mod snapshot;
mod update;

const DEFAULT_HOST: &str = "127.0.0.1";

#[derive(Parser)]
#[command(
    name = "tauri-connector",
    about = "CLI for interacting with Tauri apps",
    version = env!("CARGO_PKG_VERSION"),
)]
struct Cli {
    /// Explicit connector host (overrides discovery)
    #[arg(long, global = true)]
    host: Option<String>,
    /// Explicit connector WebSocket port (overrides discovery)
    #[arg(long, global = true)]
    port: Option<u16>,
    /// Select a discovered app by identifier
    #[arg(long, global = true)]
    app_id: Option<String>,
    /// Explicit path to .connector.json
    #[arg(long, global = true)]
    pid_file: Option<PathBuf>,
    /// Target Tauri window label
    #[arg(long, global = true, default_value = "main")]
    window_id: String,
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
        /// Max tokens for inline result (0 = unlimited, default 4000)
        #[arg(long, default_value_t = 4000)]
        max_tokens: usize,
        /// Disable subtree file splitting (full output)
        #[arg(long)]
        no_split: bool,
    },
    /// Click an element
    Click {
        /// @ref or CSS selector
        target: String,
    },
    /// Double-click an element
    Dblclick { target: String },
    /// Hover over element
    Hover { target: String },
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
    Focus { target: String },
    /// Clear and fill input
    Fill { target: String, text: Vec<String> },
    /// Type text character by character
    Type { target: String, text: Vec<String> },
    /// Check checkbox
    Check { target: String },
    /// Uncheck checkbox
    Uncheck { target: String },
    /// Select option(s) in a <select>
    Select { target: String, values: Vec<String> },
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
    Scrollintoview { target: String },
    /// Press a key (Enter, Tab, Escape, etc.)
    Press { key: String },
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
        output: Option<String>,
        /// CSS selector or @ref for an element-scoped screenshot
        #[arg(short, long)]
        selector: Option<String>,
        /// Image format: png, jpeg, webp
        #[arg(short, long, default_value = "png")]
        format: String,
        /// JPEG/WebP quality (0-100)
        #[arg(short, long, default_value_t = 80)]
        quality: u8,
        /// Max width in pixels (resize if larger)
        #[arg(short, long)]
        max_width: Option<u32>,
        /// Allow overwriting an existing output path
        #[arg(long)]
        overwrite: bool,
        /// Directory for auto-generated screenshot names
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Short slug included in auto-generated names
        #[arg(long)]
        name_hint: Option<String>,
    },
    /// Get cached DOM snapshot (pushed from frontend)
    Dom,
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
        /// What to clear: logs, ipc, events, runtime, all
        target: String,
    },
    /// Show captured frontend runtime failures and navigation/network events
    Runtime {
        /// Number of entries
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: usize,
        /// Runtime kind filter: network, window_error, unhandledrejection, navigation, resource_error
        #[arg(short, long)]
        kind: Option<String>,
        /// Level filter: error, warn, info
        #[arg(short, long)]
        level: Option<String>,
        /// Regex pattern to match
        #[arg(short, long)]
        pattern: Option<String>,
        /// Only entries since this timestamp (epoch ms)
        #[arg(long)]
        since: Option<u64>,
        /// Only entries since a debug mark
        #[arg(long)]
        since_mark: Option<String>,
    },
    /// Manage screenshot and diff artifacts
    Artifacts {
        #[command(subcommand)]
        action: ArtifactCommands,
    },
    /// Capture high-level debug context
    Debug {
        #[command(subcommand)]
        action: DebugCommands,
    },
    /// Perform one action and collect verification context
    Act {
        /// Action: click, fill, type, press, drag, hover
        action: String,
        /// Selector, @ref, or key for press
        target: Option<String>,
        /// Text for fill/type
        text: Vec<String>,
        /// Explicit key for press
        #[arg(long)]
        key: Option<String>,
        /// Drag target selector/@ref
        #[arg(long)]
        target_selector: Option<String>,
        /// Wait for selector after the action
        #[arg(long)]
        wait_selector: Option<String>,
        /// Wait for text after the action
        #[arg(long)]
        wait_text: Option<String>,
        /// Wait timeout in milliseconds
        #[arg(long, default_value_t = 5000)]
        timeout: u64,
        /// Include DOM snapshot in verification
        #[arg(long)]
        dom: bool,
        /// Include screenshot artifact in verification
        #[arg(long)]
        screenshot: bool,
        /// Include console log diff
        #[arg(long)]
        logs: bool,
        /// Include IPC diff
        #[arg(long)]
        ipc: bool,
        /// Include runtime capture diff
        #[arg(long)]
        runtime: bool,
    },
    /// App backend state
    State,
    /// Show discovered connector instances
    Status {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show internal webview bridge status
    Bridge,
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
    /// Diagnose current project setup and report what is missing
    Doctor {
        /// Emit machine-readable JSON instead of the text checklist
        #[arg(long)]
        json: bool,
        /// Skip live WebSocket/MCP reachability probes
        #[arg(long)]
        no_runtime: bool,
    },
    /// Manage Claude Code auto-detect hook
    Hook {
        #[command(subcommand)]
        action: HookCommands,
    },
    /// Manage snapshot sessions
    Snapshots {
        #[command(subcommand)]
        action: SnapshotActions,
    },
}

#[derive(Subcommand)]
enum SnapshotActions {
    /// List recent snapshots
    List,
    /// Read a subtree file from a snapshot
    Read {
        /// Snapshot UUID
        uuid: String,
        /// Subtree filename (e.g. subtree-0.txt). Defaults to layout.txt
        file: Option<String>,
    },
}

#[derive(Subcommand)]
enum ArtifactCommands {
    /// List recent artifacts
    List {
        /// Artifact kind filter, e.g. screenshot
        #[arg(long)]
        kind: Option<String>,
        /// Max entries to return
        #[arg(short, long, default_value_t = 100)]
        limit: usize,
    },
    /// Show artifact metadata, optionally including base64 content
    Show {
        /// Artifact ID or path
        artifact: String,
        /// Include base64 payload
        #[arg(long)]
        base64: bool,
    },
    /// Remove old artifacts from the manifest and disk
    Prune {
        /// Number of newest artifacts to keep
        #[arg(long, default_value_t = 50)]
        keep: usize,
        /// Artifact kind filter, e.g. screenshot
        #[arg(long)]
        kind: Option<String>,
        /// Only rewrite the manifest; do not delete files
        #[arg(long)]
        manifest_only: bool,
    },
    /// Compare two artifacts or file paths
    Compare {
        /// Before artifact ID or path
        before: String,
        /// After artifact ID or path
        after: String,
        /// Maximum allowed difference ratio
        #[arg(long, default_value_t = 0.0)]
        threshold: f64,
    },
}

#[derive(Subcommand)]
enum DebugCommands {
    /// Create a timestamp mark for later diff filters
    Mark {
        /// Optional label
        label: Option<String>,
    },
    /// Capture a high-level debug snapshot
    Snapshot {
        /// Include DOM snapshot
        #[arg(long)]
        dom: bool,
        /// Include screenshot artifact
        #[arg(long)]
        screenshot: bool,
        /// Include console logs
        #[arg(long)]
        logs: bool,
        /// Include IPC captures
        #[arg(long)]
        ipc: bool,
        /// Include Tauri event captures
        #[arg(long)]
        events: bool,
        /// Include runtime captures
        #[arg(long)]
        runtime: bool,
        /// Only captures since this timestamp (epoch ms)
        #[arg(long)]
        since: Option<u64>,
        /// Only captures since a debug mark
        #[arg(long)]
        since_mark: Option<String>,
        /// Max tokens for DOM snapshot
        #[arg(long)]
        max_tokens: Option<u64>,
        /// Name hint for screenshot artifact
        #[arg(long)]
        screenshot_name_hint: Option<String>,
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
    commands::set_window_id(cli.window_id.clone());
    let connection_options = ConnectionOptions {
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        host: cli.host.clone(),
        port: cli.port,
        app_id: cli.app_id.clone(),
        pid_file: cli.pid_file.clone(),
    };

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

    if let Commands::Doctor { json, no_runtime } = &cli.command {
        let opts = doctor::Options {
            json: *json,
            no_runtime: *no_runtime,
        };
        if let Err(e) = doctor::run(opts).await {
            eprintln!("{e}");
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

    if let Commands::Snapshots { action } = &cli.command {
        let resolved = discovery::resolve_connection(connection_options.clone())
            .await
            .ok();
        let instance = resolved.as_ref().and_then(|r| r.instance.as_ref());
        let result = match action {
            SnapshotActions::List => commands::snapshots_list(instance),
            SnapshotActions::Read { uuid, file } => {
                commands::snapshots_read(instance, uuid, file.as_deref())
            }
        };
        if let Err(e) = result {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    if let Commands::Status { json } = &cli.command {
        let host = cli
            .host
            .clone()
            .or_else(|| std::env::var("TAURI_CONNECTOR_HOST").ok())
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        let statuses = discovery::instance_statuses(
            &connection_options.cwd,
            connection_options.app_id.as_deref(),
            connection_options.pid_file.as_deref(),
            Some(&host),
        )
        .await;
        let resolved = discovery::resolve_connection(connection_options.clone())
            .await
            .ok();
        if *json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "resolved": resolved,
                    "instances": statuses,
                }))
                .unwrap_or_default()
            );
        } else {
            if let Some(r) = resolved {
                println!("connected: {}:{} ({:?})", r.host, r.port, r.source);
            } else {
                println!("connected: none");
            }
            if statuses.is_empty() {
                println!("instances: none");
            } else {
                println!("instances:");
                for s in statuses {
                    let i = s.instance;
                    let state = if s.stale { "stale" } else { "ready" };
                    println!(
                        "  {state} {}:{} pid={} app={} id={} file={}",
                        host,
                        i.ws_port,
                        i.pid,
                        i.app_name.as_deref().unwrap_or("?"),
                        i.app_id.as_deref().unwrap_or("?"),
                        i.pid_file.display(),
                    );
                    if let Some(error) = s.error {
                        println!("    {error}");
                    }
                }
            }
        }
        return;
    }

    let resolved = match discovery::resolve_connection(connection_options).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let mut client = ConnectorClient::new();

    // Connect with ping fallback
    if let Err(e) = client.connect(&resolved.host, resolved.port).await {
        eprintln!(
            "Error: Failed to connect to {}:{}: {e}",
            resolved.host, resolved.port
        );
        std::process::exit(1);
    }

    let refs = match snapshot::load_ref_cache(&resolved, &cli.window_id) {
        Ok(refs) => refs,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

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
            max_tokens,
            no_split,
        } => {
            match commands::snapshot(
                &client,
                interactive,
                compact,
                depth,
                max_elements,
                selector,
                mode,
                !no_react,
                !no_portals,
                max_tokens,
                no_split,
            )
            .await
            {
                Ok(new_refs) => {
                    if let Err(e) = snapshot::save_ref_cache(&resolved, &cli.window_id, new_refs) {
                        Err(e)
                    } else {
                        Ok(())
                    }
                }
                Err(e) => Err(e),
            }
        }
        Commands::Click { target } => commands::click(&client, &refs, &target).await,
        Commands::Dblclick { target } => commands::dblclick(&client, &refs, &target).await,
        Commands::Hover { target } => commands::hover(&client, &refs, &target).await,
        Commands::Drag {
            source,
            target,
            steps,
            duration,
            strategy,
        } => commands::drag(&client, &refs, &source, &target, steps, duration, &strategy).await,
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
        } => commands::get_prop(&client, &refs, &prop, target.as_deref(), extra.as_deref()).await,
        Commands::Wait {
            selector,
            text,
            timeout,
        } => commands::wait(&client, selector.as_deref(), text.as_deref(), timeout).await,
        Commands::Eval { script } => commands::eval_js(&client, &script.join(" ")).await,
        Commands::Logs {
            lines,
            filter,
            level,
            pattern,
        } => {
            commands::logs(
                &client,
                lines,
                filter.as_deref(),
                level.as_deref(),
                pattern.as_deref(),
            )
            .await
        }
        Commands::Screenshot {
            output,
            selector,
            format,
            quality,
            max_width,
            overwrite,
            output_dir,
            name_hint,
        } => {
            let instance = resolved.instance.as_ref();
            commands::screenshot(
                &client,
                output.as_deref(),
                selector.as_deref(),
                &format,
                quality,
                max_width,
                overwrite,
                output_dir.as_deref(),
                name_hint.as_deref(),
                instance,
            )
            .await
        }
        Commands::Dom => commands::cached_dom(&client, &cli.window_id).await,
        Commands::Find { selector, strategy } => {
            commands::find(&client, &selector, &strategy).await
        }
        Commands::Pointed => commands::pointed(&client).await,
        Commands::Resize { width, height } => {
            commands::resize(&client, &cli.window_id, width, height).await
        }
        Commands::Ipc { action } => match action {
            IpcCommands::Exec { command, args } => {
                commands::ipc_exec(&client, &command, args.as_deref()).await
            }
            IpcCommands::Monitor => commands::ipc_monitor(&client, "start").await,
            IpcCommands::Unmonitor => commands::ipc_monitor(&client, "stop").await,
            IpcCommands::Captured {
                filter,
                pattern,
                since,
                limit,
            } => {
                commands::ipc_captured(&client, filter.as_deref(), pattern.as_deref(), since, limit)
                    .await
            }
        },
        Commands::Emit { event, payload } => {
            commands::ipc_emit(&client, &event, payload.as_deref()).await
        }
        Commands::Events { action } => match action {
            EventCommands::Listen { events } => commands::event_listen(&client, &events).await,
            EventCommands::Captured {
                pattern,
                since,
                limit,
            } => commands::event_captured(&client, pattern.as_deref(), since, limit).await,
            EventCommands::Stop => commands::event_stop(&client).await,
        },
        Commands::Clear { target } => commands::clear_logs(&client, &target).await,
        Commands::Runtime {
            lines,
            kind,
            level,
            pattern,
            since,
            since_mark,
        } => {
            commands::runtime(
                &client,
                lines,
                kind.as_deref(),
                level.as_deref(),
                pattern.as_deref(),
                since,
                since_mark.as_deref(),
                &cli.window_id,
            )
            .await
        }
        Commands::Artifacts { action } => match action {
            ArtifactCommands::List { kind, limit } => {
                commands::artifacts_list(&client, kind.as_deref(), limit).await
            }
            ArtifactCommands::Show { artifact, base64 } => {
                commands::artifact_show(&client, &artifact, base64).await
            }
            ArtifactCommands::Prune {
                keep,
                kind,
                manifest_only,
            } => commands::artifact_prune(&client, keep, kind.as_deref(), !manifest_only).await,
            ArtifactCommands::Compare {
                before,
                after,
                threshold,
            } => commands::artifact_compare(&client, &before, &after, threshold).await,
        },
        Commands::Debug { action } => match action {
            DebugCommands::Mark { label } => commands::debug_mark(&client, label.as_deref()).await,
            DebugCommands::Snapshot {
                dom,
                screenshot,
                logs,
                ipc,
                events,
                runtime,
                since,
                since_mark,
                max_tokens,
                screenshot_name_hint,
            } => {
                commands::debug_snapshot(
                    &client,
                    &cli.window_id,
                    dom,
                    screenshot,
                    logs,
                    ipc,
                    events,
                    runtime,
                    since,
                    since_mark.as_deref(),
                    max_tokens,
                    screenshot_name_hint.as_deref(),
                )
                .await
            }
        },
        Commands::Act {
            action,
            target,
            text,
            key,
            target_selector,
            wait_selector,
            wait_text,
            timeout,
            dom,
            screenshot,
            logs,
            ipc,
            runtime,
        } => {
            let action_key = if action == "press" {
                key.as_deref().or(target.as_deref())
            } else {
                key.as_deref()
            };
            let action_text = if text.is_empty() {
                None
            } else {
                Some(text.join(" "))
            };
            commands::act_and_verify(
                &client,
                &action,
                if action == "press" {
                    None
                } else {
                    target.as_deref()
                },
                action_text.as_deref(),
                action_key,
                target_selector.as_deref(),
                wait_selector.as_deref(),
                wait_text.as_deref(),
                timeout,
                dom,
                screenshot,
                logs,
                ipc,
                runtime,
                &cli.window_id,
            )
            .await
        }
        Commands::State => commands::state(&client).await,
        Commands::Status { .. } => unreachable!(),
        Commands::Bridge => commands::bridge_status(&client).await,
        Commands::Windows => commands::windows(&client).await,
        Commands::Snapshots { .. } => unreachable!(),
        Commands::Update { .. } => unreachable!(),
        Commands::Examples => unreachable!(),
        Commands::Doctor { .. } => unreachable!(),
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
  Resolves the connector endpoint as:
  --host/--port > TAURI_CONNECTOR_* env > .connector.json > port scan

STATUS:
  status [--json]                    Show discovered instances and selected endpoint
  bridge                             Show connected internal webview bridge clients

SNAPSHOT:
  snapshot [-i] [-c] [-d N] [-s "#selector"] [--max-tokens N] [--no-split]
    Take DOM snapshot with ref IDs. Flags:
      -i  Interactive only (elements with refs)
      -c  Compact (remove structural wrappers)
      -d  Max depth
      -s  Scope to CSS selector
      --max-tokens  Max tokens for inline result (0=unlimited, default 4000)
      --no-split    Disable subtree file splitting (full output)

SNAPSHOTS:
  snapshots list                    List recent snapshot sessions
  snapshots read <uuid> [file]      Read a subtree file (default: layout.txt)

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
  screenshot [path] [--selector @eN] [-f png|jpeg|webp] [-q 80] [-m 1280]

ARTIFACTS:
  artifacts list [--kind screenshot]  List artifact manifest entries
  artifacts show <id|path>            Show artifact metadata
  artifacts compare <before> <after>  Compare two artifacts or paths
  artifacts prune --keep 50           Prune older artifacts

DEBUG:
  debug mark [label]                 Create a timestamp mark
  debug snapshot [--dom --logs]      Collect bundled debug context
  act click @e5 --wait-text Success  Act, wait, and collect fresh evidence
  runtime [-n 100] [-l error]        Captured runtime failures/navigation/network

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
  clear logs|ipc|events|runtime|all  Clear persisted captures
  state                              App metadata
  doctor [--json] [--no-runtime]     Diagnose current project setup
  help                               This help

EXAMPLES:
  tauri-connector snapshot -i -c
  tauri-connector click @e7
  tauri-connector fill @e5 "user@example.com"
  tauri-connector screenshot --name-hint debug -m 1280
  tauri-connector act click @e7 --wait-text Success --logs --ipc --runtime
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
