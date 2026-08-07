# LLM-API-Proxy

English | [简体中文](./README.md)

A local proxy gateway that unifies multi-vendor LLM APIs into a single OpenAI-compatible endpoint, with a native desktop GUI.

Aggregate multiple LLM providers (DeepSeek, OpenAI, Zhipu GLM, Grok, Claude, etc.) into **one OpenAI-compatible entry point** with automatic round-robin load balancing and failover — configure once in your client tools and seamlessly switch upstreams.

## 🤔 Why This Project?

If you subscribe to multiple LLM API providers (DeepSeek, OpenAI, Zhipu, Grok, Claude...), you've likely encountered these frustrations:

- **Each provider needs separate configuration** — different URLs, API keys, model names. Switching between them in ChatGPT, Claude Desktop, Cursor, and other clients is a headache
- **A provider suddenly goes down** — manually swapping keys and endpoints, then switching back when it's fixed, requires constant manual attention
- **You want to distribute quota** — sending the same query to different providers in rotation is impractical to do manually

Most aggregation solutions on the market look like this:

| Pain Point | Typical Solution | Real Experience |
|------------|------------------|-----------------|
| Complex deployment | Requires Docker / Node.js, lots of `docker-compose` configs | Beginners are scared away by the command line |
| Tedious config | Edit YAML / JSON files, manually maintain upstream lists | One wrong indent and the whole service won't start |
| Not portable | Deployed on a server or fixed machine, reconfigure on every new computer | One setup at work, one at home — never in sync |
| Requires ops | Monitor container status, logs, ports, firewalls | Using an API proxy means learning DevOps? |
| Data security | API keys stored in plaintext config files | Anyone with the file can use your quota |

**LLM-API-Proxy is built to solve all of these:**

| Advantage | This Project | What It Means for You |
|-----------|-------------|----------------------|
| **Double-click to run** | Single `.exe` file, no Docker / Node.js / Python needed | Download → Double-click → Start using. As simple as installing any app |
| **Visual GUI** | Native desktop window, fill in forms instead of editing config files | WYSIWYG, no commands to memorize |
| **USB portable** | Copy the folder to a USB drive, plug into any PC and run | Office ← → Home ← → Laptop, your config travels with you |
| **One-click model fetch** | Enter URL and Key, click once to auto-fetch available models | No need to look up model names in docs |
| **Auto failover** | If one provider is down, automatically moves to the next, fully transparent | No more waking up at night to swap keys |
| **API Key encryption** | AES-256-GCM encrypted storage, plaintext never touches disk | No worries about someone snooping your files |
| **In-app updates** | Built-in version checker, one-click download & update (GitHub + Gitee dual-source) | Smooth updates even on restricted networks |
| **Bilingual UI** | Interface supports Chinese / English toggle | Easy to use regardless of your native language |
| **Zero ops** | Close window to minimize to tray, runs silently in background | Just forget it's there |

> **In short:** If you don't want to deal with Docker, memorize commands, or reconfigure APIs on every new computer — this tool is for you.

## ✨ Core Features

- **Unified entry point** — Exposes a single OpenAI-compatible URL and API key, compatible with ChatGPT, Claude Desktop, LibreChat, FastGPT, and other mainstream clients
- **One-click tool injection** — Supports 8 AI coding tools including Claude Code, Codex CLI, Gemini CLI, etc. Injects proxy address into config files with one click
- **Multi-level thinking intensity** — Six levels (off/low/medium/high/max/custom), auto-mapped to provider-specific parameters
- **Multi-format API conversion** — Supports OpenAI Chat, Anthropic Messages, Gemini Native, and OpenAI Responses formats with automatic bidirectional conversion
- **Model capability tagging** — Auto-detects and labels model capabilities (text/image/audio input, function calling, context window, etc.)
- **Pool aggregation** — Combine multiple providers' same/similar models into a custom pool, presented as a single "model" externally
- **Round-robin** — Each request is distributed to a different upstream in rotation for load balancing
- **Failover** — When an upstream fails, automatically skips it and tries the next, supporting HTTP errors, timeouts, and response-body `error` field detection
- **SSE streaming passthrough** — Full passthrough of upstream SSE event streams, supporting streaming chat and token usage tracking
- **Model aliasing** — Client tools always see the pool name; the actual upstream model name is hidden
- **AES-256-GCM encryption** — API keys encrypted and stored in SQLite, Master Key follows the data directory, USB-plug-and-play
- **Request logging** — Records status code, latency, token usage, and failed upstream chain for each request, with filtering by status code/time and export
- **Multi-key access control** — Create multiple Gateway API Keys with configurable pool-level permissions and expiration
- **Rate limiting** — Per-IP rate limiting (configurable window size and request count), state persisted across restarts
- **Upstream health tracking** — Auto-records failure count, error reason, recovery time; status tiers (healthy/degraded/down); supports background probing
- **Database backup & restore** — One-click database backup, restore from backup file (requires restart), supports automatic scheduled backups
- **Config import/export** — Export all upstreams, pools, and settings as JSON; supports incremental and full import modes
- **Diagnostic package** — Export a ZIP containing version info, config summary, upstream status, and recent logs; sensitive data automatically masked
- **Alert monitoring** — Background monitoring of request failure rate; triggers alerts when threshold exceeded; supports silence period to prevent fatigue
- **Health check** — One-click connectivity test for all upstreams
- **In-app updates** — Automatically checks for new versions on startup, supports GitHub + Gitee dual data sources, one-click download with automatic replacement and restart
- **Bilingual UI** — Interface supports Chinese / English switching, system tray menu follows language setting
- **System tray** — Close window to minimize to tray, proxy service continues in background
- **Light/Dark theme** — Supports light/dark mode toggle with persistence

## 🛠 Tech Stack

| Module | Technology |
|--------|-----------|
| Backend framework | Axum 0.8 (Rust) |
| Desktop GUI | Tauri v2 |
| Database | SQLite (rusqlite, WAL mode) |
| Encryption | AES-256-GCM (aes-gcm) |
| HTTP client | reqwest 0.12 |
| Async runtime | tokio |
| Logging | tracing + tracing-subscriber |
| Frontend | Vanilla HTML/CSS/JS (embedded in Tauri Webview) |

## 📁 Project Structure

```
.
├── Cargo.toml              # Workspace root config
├── dev-server.js           # Dev-mode static file server
├── package.json
├── src/                    # Rust core library (backend logic)
│   ├── lib.rs              # Module declarations + AppState + backend init
│   ├── main.rs             # Standalone mode entry (no GUI)
│   ├── config.rs           # Gateway config & path management
│   ├── config_io.rs        # Config import/export (JSON format)
│   ├── crypto.rs           # AES-256-GCM encryption/decryption + Master Key management
│   ├── diagnostic.rs       # Diagnostic package (ZIP export + sensitive data masking)
│   ├── error.rs            # Unified error types
│   ├── db/                 # SQLite data layer (modular)
│   │   ├── mod.rs          # Database wrapper + read-write separation + transactions
│   │   ├── migration.rs     # Schema creation + migrations (idempotent)
│   │   ├── upstream.rs     # Upstream CRUD + health status updates
│   │   ├── pool.rs         # Pool CRUD
│   │   ├── log.rs          # Request logs + stats + percentile calculation
│   │   ├── settings.rs     # Key-value settings storage
│   │   ├── api_key.rs      # Multi-key management
│   │   ├── backup.rs       # Database backup & restore
│   │   ├── rate_limit.rs   # Rate limit state persistence
│   │   ├── token_usage.rs  # Standalone token usage stats
│   │   └── tool_config.rs  # Tool config switch data layer
│   ├── gateway/            # OpenAI-compatible gateway
│   │   ├── mod.rs          # /v1/models, /v1/chat/completions routes
│   │   ├── auth.rs         # Multi-key auth (constant-time comparison + pool permissions)
│   │   ├── stream.rs       # SSE streaming (model replacement + usage extraction + error detection)
│   │   ├── rate_limit.rs   # Rate limiter (DashMap + persistence)
│   │   ├── error_response.rs # OpenAI-compatible error responses
│   │   ├── health.rs       # Three-tier health check
│   │   └── convert/        # Multi-format API conversion
│   │       ├── mod.rs      # Conversion module entry + format detection
│   │       ├── anthropic.rs # Anthropic Messages ↔ Chat
│   │       ├── gemini.rs   # Gemini Native ↔ Chat
│   │       ├── openai_responses.rs # OpenAI Responses ↔ Chat
│   │       └── capabilities.rs # Model capability inference
│   ├── pool/               # Pool & round-robin logic
│   │   ├── mod.rs          # Pool data structures
│   │   └── thinking.rs     # Multi-level thinking parameter injection (by provider)
│   ├── tool_config/        # Tool config injection module
│   │   ├── mod.rs          # Module entry + switch manager
│   │   ├── detector.rs     # Tool installation detection
│   │   ├── backup.rs       # Config backup & restore
│   │   ├── writer.rs       # Atomic write engine
│   │   ├── env_check.rs    # Env var conflict detection & cleanup
│   │   ├── claude.rs       # Claude Code writer
│   │   ├── claude_desktop.rs # Claude Desktop writer
│   │   ├── codex.rs        # Codex CLI writer
│   │   ├── gemini.rs       # Gemini CLI writer
│   │   ├── grok.rs         # Grok CLI writer
│   │   ├── opencode.rs     # OpenCode writer
│   │   ├── openclaw.rs     # OpenClaw writer
│   │   └── hermes.rs       # Hermes writer
│   ├── proxy/              # Upstream forwarding
│   │   ├── mod.rs          # Module declarations
│   │   ├── failover.rs     # HTTP forwarding & failover chain
│   │   └── error.rs        # Structured upstream error classification
│   ├── probe/              # Background upstream probing
│   │   └── mod.rs          # Periodic probing + health status updates
│   ├── alert/              # Alert threshold monitoring
│   │   └── mod.rs          # Failure rate monitoring + silence period
│   └── tests/              # Integration tests
│       ├── common/mod.rs   # Test utilities
│       └── integration/    # Gateway + streaming integration tests
├── src-tauri/              # Tauri desktop app shell
│   ├── tauri.conf.json     # Window / build config
│   ├── capabilities/       # Tauri permission config
│   ├── icons/              # App icons
│   └── src/
│       ├── lib.rs          # Tauri app entry + system tray + window events
│       ├── main.rs         # main entry
│       └── commands/       # Tauri commands (modular)
│           ├── mod.rs      # Shared DTOs + ID generation
│           ├── upstream.rs # Upstream management commands
│           ├── pool.rs     # Pool management commands
│           ├── log.rs      # Log & stats commands
│           ├── settings.rs # Settings commands
│           ├── health.rs   # Health check commands
│           ├── api_key.rs  # Multi-key management commands
│           ├── backup.rs   # Backup & restore commands
│           ├── diagnostic.rs # Diagnostic export commands
│           ├── update.rs   # In-app update commands
│           ├── shortcut.rs # Shortcut commands
│           └── tool_config.rs # Tool config commands
└── dist/                   # Frontend build output
    └── index.html          # Single-page app (embedded CSS/JS)
```

## 🚀 Quick Start

### Prerequisites

- **Rust** 1.77+ (latest stable recommended)
- **Tauri CLI** 2.0+
- **Node.js** 18+ (only needed for dev mode)
- **Windows** 10/11 (currently Windows-only)

### Development Mode

```bash
# 1. Install Tauri CLI
cargo install tauri-cli --version "^2"

# 2. Start dev server (frontend hot-reload + Rust hot-compile)
cargo tauri dev
```

### Build for Release

```bash
# Build portable exe (generates a standalone exe, no installer)
cargo tauri build
```

The build output is at `target/release/llm-api-proxy-app.exe`. For release, rename it to `LLM-API-Proxy_vX.Y.Z_x64_portable.exe`.

## 📖 Usage Guide

### 1. Launch

Double-click the `.exe` — the GUI window opens automatically and the API gateway service starts in the background (listening on `127.0.0.1:47339` by default). The Gateway API Key is auto-generated on first launch and can be viewed on the "Gateway Settings" page.

### 2. Add Upstream Subscription

Go to the "Upstream Management" page and click "Add":

1. Fill in provider name, Base URL, and API Key
2. Click "Fetch Model List" to retrieve available models from the provider
3. Select the target model and save

The API key is encrypted with AES-256-GCM before being stored in the database — plaintext never touches disk.

> Optional: Set the upstream's native API format (OpenAI / Anthropic / Gemini) — the gateway handles format conversion automatically. Capability tags are auto-inferred from the model name when left empty.

### 3. Create a Pool

Go to the "Pool Management" page and create a pool:

1. Enter a pool name (e.g., `grok-4.5` — this is the model name your client tools will see)
2. Associate one or more upstream subscriptions
3. Optional: Set thinking intensity (off/low/medium/high/max/custom), max concurrency, failover

### 4. Connect Client Tools

Configure any OpenAI-compatible client:

```
Base URL: http://127.0.0.1:47339/v1
API Key:  sk-gw-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
Model:    grok-4.5
```

That's it. Requests are automatically distributed across upstreams in the pool, with automatic failover when a provider is down.

### 5. Configure Tools (Optional)

Go to the "Tool Config" page to inject the proxy address into installed AI coding tools:

1. The page auto-detects installed tools (Claude Code, Codex CLI, etc. — 8 tools supported)
2. Toggle a tool's switch, select a default pool and API Key in the dialog
3. Confirm to write the config file — the tool can now use the proxy directly

> Original configs are automatically backed up; toggling off or exiting the app restores the original.

### 6. Minimize to Tray

Closing the window minimizes the app to the system tray — the proxy service continues running in the background. Double-click the tray icon or right-click "Open Main Window" to restore.

### 7. Version Updates

The app automatically checks for the latest release on GitHub / Gitee at startup. When a new version is found:

1. A red dot appears on the "Check for Updates" button in the sidebar
2. Go to the "Settings" page — new version info and changelog are displayed automatically
3. Click "GitHub Download" or "Gitee Download" — the app downloads the new version, replaces the executable, and restarts automatically
4. Desktop shortcuts remain valid after update (Windows icon cache is refreshed automatically)

> Users in China are recommended to use the Gitee download for faster speeds.

## 💬 Community & Feedback

Join our QQ group for discussion, feedback, and feature suggestions:

![QQ Group QR Code](./images/qrcode_1785163779542.jpg)

## ⭐ Star This Project

If this project has been helpful to you, please consider giving it a Star ⭐. Your support helps more people discover this project. Thank you!

## 🔌 API Reference

### OpenAI-Compatible Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v1/models` | GET | Returns all pool names as the available model list |
| `/v1/chat/completions` | POST | Forwards request to pool upstreams, supports streaming/non-streaming |
| `/v1/messages` | POST | Anthropic Messages format endpoint, compatible with Claude native clients |
| `/v1beta/models/{model}:generateContent` | POST | Gemini Native format endpoint, compatible with Gemini native clients |
| `/v1/responses` | POST | OpenAI Responses format endpoint, compatible with Codex CLI and similar tools |
| `/api/health` | GET | Health check |

**Request example:**

```http
POST /v1/chat/completions
Authorization: Bearer sk-gw-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
Content-Type: application/json

{
  "model": "grok-4.5",
  "messages": [{"role": "user", "content": "Hello"}],
  "stream": true
}
```

### Failover Behavior

- Upstream returns HTTP 5xx or connection timeout → automatically tries the next upstream
- Upstream returns HTTP 200 but response body contains an `error` field → also triggers failover
- Upstream auth failure (401/403) → triggers failover (different upstreams have different keys)
- 4xx client errors (400/404 etc.) → does not trigger failover (request itself is the problem)
- All upstreams fail → returns `502 Bad Gateway` with error details for each failed upstream
- No available upstream in pool (all disabled) → returns `503 Service Unavailable`

### Rate Limiting Behavior

- Each IP can send at most a configured number of requests within a configured time window (default 60/min)
- When exceeded, returns `429 Too Many Requests` with a `Retry-After` header
- Rate limit state is persisted to the database, survives restarts
- Supports reverse proxy mode (identifies real client IP via `X-Forwarded-For`)

## 🔒 Security Design

| Measure | Description |
|---------|-------------|
| Local-only binding | Binds to `127.0.0.1` by default, not exposed to the internet |
| Encrypted API key storage | AES-256-GCM encryption, Master Key stored in local file, plaintext never on disk |
| Multi-key access control | Supports multiple Gateway API Keys with configurable pool permissions and expiration |
| Constant-time comparison | API key validation uses constant-time comparison to prevent timing side-channel attacks |
| Rate limiting | Per-IP rate limiting (configurable window and request count), supports reverse proxy X-Forwarded-For |
| Read-write separation | SQLite WAL mode + dedicated read-only connection, SELECTs don't block writes |
| XSS protection | Dynamic frontend content is HTML-escaped |
| Command injection protection | External links are validated against a URL protocol whitelist before opening |
| Response header filtering | Only whitelisted upstream response headers are passed through, preventing internal info leakage |
| Transaction isolation | Database writes use transactions + Mutex locks to prevent concurrent interference |
| UUID identifiers | Request logs use UUID v4 for IDs, preventing collisions |

## 📦 Portable Deployment

The program and all data follow the exe's directory — no runtime dependencies to install:

```
LLM-API-Proxy/
├── LLM-API-Proxy.exe        # Main program
└── data/                    # All data travels with the directory
    ├── proxy.db             # SQLite database (config + logs)
    └── master_key.bin       # AES master key
```

- Copy the entire folder to a USB drive, plug into any Windows PC and run
- Delete the folder for a complete uninstall — no registry entries, no system services

## 📝 License

This project uses a **dual-license model**.

### Default Open Source License: AGPL-3.0-only

This project is open-sourced under [GNU AGPL-3.0](./LICENSE) by default. You may freely use, modify, and distribute it, but derivative works must be open-sourced under the same license, and source code must be disclosed even when providing services over a network.

### Commercial License

The following **service-oriented commercial uses** require prior commercial authorization from the project maintainer:

- Using this project (or a modified version) as a backend or core service to provide SaaS, hosting, or management services (paid or free) to third parties
- Integrating this project into a commercial product for distribution or sale
- Internal production use by enterprises, organizations, teams, or other non-individual entities

| Use Case | License Requirement |
|----------|-------------------|
| Personal study / research / daily use | ✅ Free under AGPL-3.0 |
| Modified for personal use (not distributed, no service provided) | ✅ Free under AGPL-3.0 |
| Open-source community contributions (Issues / PRs) | ✅ Welcome! See [CONTRIBUTING.md](./CONTRIBUTING.md) |
| Distribution of modified versions (open-source derivatives) | ✅ Must open-source derivatives under AGPL-3.0 |
| Service-oriented commercial use (SaaS / hosting / enterprise internal) | ⚠️ Requires commercial license |

> **In short: Personal and non-commercial use is free under AGPL-3.0; for commercial service use, please contact the maintainer for authorization in advance.**
>
> For commercial licensing, contact the project maintainer via GitHub Issues.

### Contributor Agreement

Submitting a Pull Request indicates your agreement with the terms in [CLA.md](./CLA.md). You retain copyright of your contributions, but grant the project maintainer the right to use them in commercial releases.
