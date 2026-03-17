//! tauri-connector CLI — interact with Tauri apps from the terminal.

use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use connector_client::ConnectorClient;

mod commands;
mod snapshot;

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
#[command(name = "tauri-connector", about = "CLI for interacting with Tauri apps")]
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
        /// Max depth
        #[arg(short, long, default_value_t = 0)]
        depth: usize,
        /// Scope to CSS selector
        #[arg(short, long)]
        selector: Option<String>,
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
    },
    /// App backend state
    State,
    /// List windows
    Windows,
    /// Show detailed help with examples
    Examples,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if matches!(cli.command, Commands::Examples) {
        print_help();
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
            selector,
        } => {
            match commands::snapshot(&client, interactive, compact, depth, selector).await {
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
        Commands::Logs { lines, filter } => {
            commands::logs(&client, lines, filter.as_deref()).await
        }
        Commands::State => commands::state(&client).await,
        Commands::Windows => commands::windows(&client).await,
        Commands::Examples => unreachable!(),
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

OTHER:
  eval <js-expression>               Execute JavaScript
  logs [-n 20] [-f "filter"]         Console logs
  state                              App metadata
  windows                            List windows
  help                               This help

EXAMPLES:
  tauri-connector snapshot
  tauri-connector snapshot -i -c
  tauri-connector click @e7
  tauri-connector fill @e5 "user@example.com"
  tauri-connector get text @e4
  tauri-connector press Enter
  tauri-connector eval "document.title"
"##
    );
}
