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
- **Pool aggregation** — Combine multiple providers' same/similar models into a custom pool, presented as a single "model" externally
- **Round-robin** — Each request is distributed to a different upstream in rotation for load balancing
- **Failover** — When an upstream fails, automatically skips it and tries the next, supporting HTTP errors, timeouts, and response-body `error` field detection
- **SSE streaming passthrough** — Full passthrough of upstream SSE event streams, supporting streaming chat and token usage tracking
- **Thinking mode** — Automatically injects `reasoning` / `reasoning_effort` / `thinking` parameters based on upstream provider type
- **Model aliasing** — Client tools always see the pool name; the actual upstream model name is hidden
- **AES-256-GCM encryption** — API keys encrypted and stored in SQLite, Master Key follows the data directory, USB-plug-and-play
- **Request logging** — Records status code, latency, token usage, and failed upstream chain for each request, with filtering by status code/time and export
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
│   ├── config.rs           # Gateway config & path management
│   ├── crypto.rs           # AES-256-GCM encryption/decryption + Master Key management
│   ├── db.rs               # SQLite data layer (CRUD + migrations + transactions)
│   ├── error.rs            # Unified error types
│   ├── gateway/            # OpenAI-compatible gateway
│   │   ├── mod.rs          # /v1/models, /v1/chat/completions routes
│   │   ├── auth.rs         # API Key authentication (constant-time comparison)
│   │   └── stream.rs       # SSE streaming (model name replacement + token extraction)
│   ├── pool/               # Pool & round-robin logic
│   │   ├── mod.rs          # Pool data structures
│   │   ├── round_robin.rs  # Round-robin selector
│   │   └── thinking.rs     # Thinking mode parameter injection (by provider)
│   └── proxy/              # Upstream forwarding
│       ├── mod.rs          # ProxyEngine
│       ├── client.rs       # UpstreamConfig definition
│       ├── failover.rs     # HTTP forwarding & failover chain
│       └── model_filter.rs # Model name replacement
├── src-tauri/              # Tauri desktop app shell
│   ├── tauri.conf.json     # Window / build config
│   ├── capabilities/       # Tauri permission config
│   ├── icons/              # App icons
│   └── src/
│       ├── lib.rs          # Tauri app entry + system tray + window events
│       ├── main.rs         # main entry
│       └── commands.rs     # Tauri commands (GUI ↔ backend bridge)
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

### 3. Create a Pool

Go to the "Pool Management" page and create a pool:

1. Enter a pool name (e.g., `grok-4.5` — this is the model name your client tools will see)
2. Associate one or more upstream subscriptions
3. Optional: enable thinking mode, set max concurrency, configure failover

### 4. Connect Client Tools

Configure any OpenAI-compatible client:

```
Base URL: http://127.0.0.1:47339/v1
API Key:  sk-gw-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
Model:    grok-4.5
```

That's it. Requests are automatically distributed across upstreams in the pool, with automatic failover when a provider is down.

### 5. Minimize to Tray

Closing the window minimizes the app to the system tray — the proxy service continues running in the background. Double-click the tray icon or right-click "Open Main Window" to restore.

### 6. Version Updates

The app automatically checks for the latest release on GitHub / Gitee at startup. When a new version is found:

1. A red dot appears on the "Check for Updates" button in the sidebar
2. Go to the "Settings" page — new version info and changelog are displayed automatically
3. Click "GitHub Download" or "Gitee Download" — the app downloads the new version, replaces the executable, and restarts automatically
4. Desktop shortcuts remain valid after update (Windows icon cache is refreshed automatically)

> Users in China are recommended to use the Gitee download for faster speeds.

## 🔌 API Reference

### OpenAI-Compatible Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v1/models` | GET | Returns all pool names as the available model list |
| `/v1/chat/completions` | POST | Forwards request to pool upstreams, supports streaming/non-streaming |
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
- All upstreams fail → returns `502 Bad Gateway` with error details for each failed upstream
- No available upstream in pool (all disabled) → returns `503 Service Unavailable`

## 🔒 Security Design

| Measure | Description |
|---------|-------------|
| Local-only binding | Binds to `127.0.0.1` by default, not exposed to the internet |
| Encrypted API key storage | AES-256-GCM encryption, Master Key stored in local file, plaintext never on disk |
| Constant-time comparison | API key validation uses constant-time comparison to prevent timing side-channel attacks |
| XSS protection | Dynamic frontend content is HTML-escaped |
| Command injection protection | External links are validated against a URL protocol whitelist before opening |
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
