# NetworkDataShare

A small web app to share text, images and files between devices in the local network.
Open the page on two devices, send an invite from one, accept it on the other, and a chat-like session opens.

## Features

- List of all devices that have the page open, shown by their entered name or by browser, OS and IP address
- Share invites that the other side accepts or denies, with a configurable timeout
- Several sessions at once, each in its own tab
- Text messages with clickable links and a copy button, texts above 1 MB are sent as a `.txt` file
- Files of any size, sent with the attach button or by drag and drop
- Images pasted straight from the clipboard with `Ctrl+V`, PNG, JPEG, GIF, WebP, AVIF and BMP shown inline and copyable
- Sound for incoming invites, plus a browser notification for invites and messages while the tab is in the background
- Light and dark theme following the operating system

Nothing is stored.
Messages only live in the two open browser tabs, and files are piped from the sender to the receiver through the server without touching its disk,
so a session leaves no trace on the server once it ends.
Reloading a tab keeps its sessions, but clears that tab's chat history and its offered files, because they only exist in the page.

## Build

Requires Rust and Node.js.

```bash
cargo build --release
```

`build.rs` runs `npm ci` when the dependencies are missing or older than `package-lock.json`,
then builds the frontend into Cargo's `OUT_DIR` and embeds it into the binary,
so the result is a single executable and a stale `web/dist` can never end up in it.
Set `NDS_SKIP_WEB_BUILD=1` to skip npm and embed an existing `web/dist` instead, for example when it was built separately.

## Run

```bash
./target/release/network-data-share
```

The server prints the URLs it is reachable at, by default `https://<ip>:8080`.
`--help` lists all options, and `--info` prints a short description of the application and the configuration in effect.

HTTPS is the default because browsers only allow copying to the clipboard and showing notifications on secure pages.
On first start a self-signed certificate for `localhost`, the hostname and all local IP addresses is generated.
It is stored in the user's local data directory, on Linux `~/.local/share/network-data-share`,
so every device only has to accept the browser warning once, independent of the directory the server is started from.
A new certificate is only generated when it is about to expire or the machine has a host name or IP address it does not cover yet,
and it keeps the earlier names, so a VPN that comes and goes does not cause a new warning each time.
A plain `http://` request on the same port is redirected to `https://`.

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--bind` | `NDS_BIND` | `0.0.0.0:8080` | Address and port to listen on |
| `--invite-timeout` | `NDS_INVITE_TIMEOUT` | `60` | Seconds until an unanswered invite expires |
| `--reconnect-grace` | `NDS_RECONNECT_GRACE` | `10` | Seconds a disconnected client may take to reconnect before its sessions end |
| `--http` | `NDS_HTTP` | off | Serve plain HTTP, clipboard copy and notifications then only work on `localhost` |
| `--cert`, `--key` | `NDS_CERT`, `NDS_KEY` | generated | Own PEM certificate and key instead of the self-signed one |
| `--data-dir` | `NDS_DATA_DIR` | `~/.local/share/network-data-share` | Where the generated certificate is stored |
| `--max-clients-per-ip` | `NDS_MAX_CLIENTS_PER_IP` | `32` | Maximum number of clients (browser tabs) per IP address |
| `--config` | `NDS_CONFIG` | `~/.config/network-data-share/config.toml` | Config file to read, or with `--setup` to write |

A value is taken from the command line first, then from the environment variable, then from the config file,
so a single run can override the stored configuration without editing it.
The config file uses the option names without the leading dashes as keys, for example `invite-timeout = 30`.
An unknown key is an error, because a typo would otherwise silently fall back to the default.

## Setup

```bash
./target/release/network-data-share --setup --bind 0.0.0.0:9090
```

`--setup` takes the same options as a normal run and writes them to the config file, with the defaults for every option not passed,
so the file always describes the complete configuration instead of a mix with an older setup.
It then generates the certificate, so the first start does not have to,
and installs, enables and starts the systemd user service `network-data-share.service`,
which runs the binary at its current location with that config file.
Running `--setup` again overwrites the config file and restarts the service with the new configuration.
Add `--no-service` to only write the config file and the certificate, for example on systems without systemd.
`--dry-run` prints the config file and service unit that would be written and the `systemctl` commands that would run,
without changing anything, so a setup can be checked before it is applied.
`--info` ends with a `--setup` command that reproduces the configuration currently in effect,
as a starting point to adjust instead of assembling the options by hand.
A user service stops when the user logs out, `loginctl enable-linger` keeps it running.

## How it works

The browser keeps a WebSocket to `/ws` for the device list, invites, sessions and text messages.
A session ends when either side ends it, or when one side stays disconnected for longer than the reconnect grace period,
so a short network hiccup or a page reload does not end it.
A reconnect takes over the old connection even if the server has not noticed yet that it is dead,
and a duplicated browser tab detects that the copied identity is still in use and registers as a new client.

A file is first only announced to the receiver.
When the receiver downloads it, the server asks the sender to upload it and forwards the upload chunk by chunk into the download response.
The receiver's browser saves it to disk like any other download, which keeps memory use flat for files of any size.
The sender's tab has to stay open until the download is finished, because the file is read from the sender's disk on demand.
Received images up to 32 MB are fetched automatically to show them inline.
SVG images are never previewed, because they can contain script.

Downloads use a single-use ticket that the browser requests over the WebSocket,
so the client's long-lived token never appears in a URL or the browser history.
The page is served with a Content Security Policy that only allows scripts from the server,
and WebSocket connections from other websites are rejected.
A client that does not read its messages is not allowed to grow the server's memory,
messages to it are refused and it is disconnected when it falls too far behind.

## Development

```bash
cargo run -- --bind 127.0.0.1:8080
cd web && npm run dev
```

The Vite dev server proxies `/ws` and `/api` to `https://localhost:8080`, set `NDS_BACKEND` to point it elsewhere.
`npm run check` type-checks the frontend.
