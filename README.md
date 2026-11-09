# NetworkDataShare

A small web app to share text, images and files between devices in the local network.
Open the page on two devices, send an invite from one, accept it on the other, and a chat-like session opens.

## Features

- List of all devices that have the page open, shown by their entered name or by browser, OS and IP address
- Share invites that the other side accepts or denies, with a configurable timeout
- Several sessions at once, each in its own tab
- Text messages with clickable links and a copy button
- Files of any size, sent with the attach button or by drag and drop
- Images pasted straight from the clipboard with `Ctrl+V`, shown inline and copyable
- Sound and browser notification for incoming invites while the tab is in the background
- Light and dark theme following the operating system

Nothing is stored.
Messages only live in the two open browser tabs, and files are piped from the sender to the receiver through the server without touching its disk,
so a session leaves no trace once it ends.

## Build

Requires Rust and Node.js.

```bash
cargo build --release
```

`build.rs` runs `npm install` and `npm run build` in `web/`, and the built frontend is embedded into the binary,
so the result is a single executable without any files next to it.
Set `NDS_SKIP_WEB_BUILD=1` to skip the frontend build, for example when `web/dist` was built separately.

## Run

```bash
./target/release/network-data-share
```

The server prints the URLs it is reachable at, by default `https://<ip>:8080`.

HTTPS is the default because browsers only allow copying to the clipboard and showing notifications on secure pages.
On first start a self-signed certificate for `localhost`, the hostname and all local IP addresses is generated and stored in `data/`,
so every device only has to accept the browser warning once.
A plain `http://` request on the same port is redirected to `https://`.

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--bind` | `NDS_BIND` | `0.0.0.0:8080` | Address and port to listen on |
| `--invite-timeout` | `NDS_INVITE_TIMEOUT` | `60` | Seconds until an unanswered invite expires |
| `--reconnect-grace` | `NDS_RECONNECT_GRACE` | `10` | Seconds a disconnected client may take to reconnect before its sessions end |
| `--http` | `NDS_HTTP` | off | Serve plain HTTP, clipboard copy and notifications then only work on `localhost` |
| `--cert`, `--key` | `NDS_CERT`, `NDS_KEY` | generated | Own PEM certificate and key instead of the self-signed one |
| `--data-dir` | `NDS_DATA_DIR` | `data` | Where the generated certificate is stored |

## How it works

The browser keeps a WebSocket to `/ws` for the device list, invites, sessions and text messages.
A session ends when either side ends it, or when one side stays disconnected for longer than the reconnect grace period,
so a short network hiccup or a page reload does not end it.

A file is first only announced to the receiver.
When the receiver downloads it, the server asks the sender to upload it and forwards the upload chunk by chunk into the download response.
The receiver's browser saves it to disk like any other download, which keeps memory use flat for files of any size.
The sender's tab has to stay open until the download is finished, because the file is read from the sender's disk on demand.
Received images up to 32 MB are fetched automatically to show them inline.

## Development

```bash
cargo run -- --bind 127.0.0.1:8080
cd web && npm run dev
```

The Vite dev server proxies `/ws` and `/api` to `https://localhost:8080`, set `NDS_BACKEND` to point it elsewhere.
`npm run check` type-checks the frontend.
