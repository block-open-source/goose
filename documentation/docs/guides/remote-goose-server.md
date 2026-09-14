---
sidebar_position: 90
title: Running a Remote goose Server
sidebar_label: Remote Server
---

# Running a Remote goose Server

goose Desktop normally runs its own `goose serve` ACP server process in the background on the same machine. You can also run `goose serve` separately — for example, on a remote VM or a different machine on your network — and point goose Desktop at it.

This is useful when you want goose to run somewhere with more compute, a stable IP, or shared access, while still driving it from a local Desktop UI.

This guide covers:

1. [Starting a `goose serve` server on a remote machine](#1-start-the-goose-serve-server)
2. [Verifying it is reachable](#2-verify-the-server-is-up)
3. [Locating the certificate fingerprint](#3-find-the-certificate-fingerprint)
4. [Configuring goose Desktop to connect to it](#4-configure-goose-desktop)
5. [Running `goose serve` as a background service on macOS](#running-goose-serve-as-a-background-service-macos)
6. [Troubleshooting](#troubleshooting)

:::warning Use TLS for remote servers
goose Desktop accepts both HTTP and HTTPS external backend URLs, but TLS is strongly recommended when connecting over a network. Certificate fingerprint pinning requires HTTPS.
:::

## Initial Setup

### 1. Start the `goose serve` server

On the remote machine, launch `goose serve` with the host, port, TLS, and a shared secret key:

```bash
GOOSE_SERVER__SECRET_KEY='YOUR_SECRET' \
goose serve --platform desktop --enable-scheduler --host 0.0.0.0 --port 3000 --tls
```

If you are using the binary bundled with the macOS app, the command path is `/Applications/Goose.app/Contents/Resources/bin/goose`.

| Setting | Purpose |
|---------|---------|
| `--host` | Interface to bind to. Use `0.0.0.0` to accept connections from other machines. Binding to `localhost` or `127.0.0.1` will only accept local connections. |
| `--port` | TCP port to listen on. |
| `--tls` / `GOOSE_TLS=true` | Enables TLS. Strongly recommended for remote servers and required for certificate fingerprint pinning. |
| `GOOSE_SERVER__SECRET_KEY` | Shared secret. The client must send this to the ACP endpoint. Treat it like a password. |

:::tip
Pick a long, random value for `GOOSE_SERVER__SECRET_KEY` and store it in a password manager — the same value goes into goose Desktop later.
:::

### 2. Verify the server is up

First, confirm `goose serve` is actually listening on the port you expect:

```bash
lsof -nP -iTCP:3000 -sTCP:LISTEN
```

Then test the server from the server itself. The `-k` flag tells `curl` to accept the self-signed TLS certificate that `goose serve` generates:

```bash
# Connectivity only
curl -i https://127.0.0.1:3000/status -k

# ACP endpoint auth check. A 401 means the secret was rejected.
curl -i https://127.0.0.1:3000/acp -k \
  -H 'X-Secret-Key: YOUR_SECRET'
```

A successful `/status` response confirms that TLS is up. The `/acp` check should not return `401` when the secret is correct.

If you intend to reach the server from another machine, also test from there using the server's hostname or VPN address — not `127.0.0.1`.

### 3. Optionally find the certificate fingerprint

When `goose serve` runs with TLS, it generates or loads a TLS certificate. goose Desktop can pin that certificate by SHA-256 fingerprint. If you leave the fingerprint field empty, goose Desktop uses trust-on-first-use and pins the first certificate it sees for that backend.

When TLS is enabled, `goose serve` logs the fingerprint on startup. It looks like:

```text
GOOSED_CERT_FINGERPRINT=AA:BB:CC:DD:EE:FF:...
```

To capture it, either:

- Run `goose serve` interactively and read it from the terminal output, or
- Tail the log file you redirect to when running as a service (see [Running `goose serve` as a background service](#running-goose-serve-as-a-background-service-macos)):

```bash
grep GOOSED_CERT_FINGERPRINT ~/Library/Logs/GooseExternal/goose-serve.out.log
```

Make a note of the fingerprint if you want to pin a specific certificate in goose Desktop.

:::note
The fingerprint changes whenever `goose serve` regenerates its certificate (for example, if you delete the cert file). If goose Desktop suddenly refuses to connect after a server restart, re-check the fingerprint.
:::

### 4. Configure goose Desktop

On the client machine, open goose Desktop and navigate to **Settings → goose Server**:

| Setting | Value |
|---------|-------|
| **Use external server** | Enabled |
| **URL** | `https://your-server-host:3000` (use the hostname or IP that the client can reach — for example a VPN/tailnet address) |
| **Secret Key** | The same value you used for `GOOSE_SERVER__SECRET_KEY` |
| **Certificate Fingerprint** | Optional. Use the `GOOSED_CERT_FINGERPRINT` value from the server logs to pin a specific TLS certificate. |

After saving, goose Desktop will route all backend requests to the remote `goose serve` process. If the connection fails, see [Troubleshooting](#troubleshooting).

## Running `goose serve` as a Background Service (macOS)

Running `goose serve` in a terminal session is fine for testing, but for everyday use you probably want it managed as a background service so it starts at login and restarts on failure. On macOS, this is done with `launchd`.

The example below uses the CLI binary bundled with goose Desktop. If you installed the CLI separately, replace `/Applications/Goose.app/Contents/Resources/bin/goose` with the absolute path returned by `which goose`.

Because the secret is stored in XML, use a hexadecimal value such as the output of `openssl rand -hex 32`, or escape XML special characters in the value.

Create a LaunchAgent plist at `~/Library/LaunchAgents/com.goose.serve.external.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.goose.serve.external</string>

    <key>ProgramArguments</key>
    <array>
      <string>/Applications/Goose.app/Contents/Resources/bin/goose</string>
      <string>serve</string>
      <string>--platform</string>
      <string>desktop</string>
      <string>--enable-scheduler</string>
      <string>--host</string>
      <string>0.0.0.0</string>
      <string>--port</string>
      <string>3000</string>
      <string>--tls</string>
    </array>

    <key>EnvironmentVariables</key>
    <dict>
      <key>GOOSE_SERVER__SECRET_KEY</key><string>YOUR_SECRET</string>
    </dict>

    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>

    <key>StandardOutPath</key>
    <string>/Users/YOUR_USERNAME/Library/Logs/GooseExternal/goose-serve.out.log</string>
    <key>StandardErrorPath</key>
    <string>/Users/YOUR_USERNAME/Library/Logs/GooseExternal/goose-serve.err.log</string>
  </dict>
</plist>
```

Replace `YOUR_SECRET` and `YOUR_USERNAME` with appropriate values. Then create the log directory and restrict access to the plist, which contains the secret key:

```bash
mkdir -p ~/Library/Logs/GooseExternal
chmod 600 ~/Library/LaunchAgents/com.goose.serve.external.plist
```

Load and start the service:

```bash
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.goose.serve.external.plist
```

To restart the service later:

```bash
launchctl kickstart -k gui/$(id -u)/com.goose.serve.external
```

To stop and unload the service:

```bash
launchctl bootout gui/$(id -u)/com.goose.serve.external
```

To remove the service configuration, delete the plist after unloading it.

## Troubleshooting

### Server only accepts local connections

If `curl` works from the server but the client machine times out or gets "connection refused", check what interface `goose serve` is bound to. If `--host` is `localhost` or `127.0.0.1`, only loopback connections are accepted.

Set `--host 0.0.0.0` to accept connections on all interfaces, then restart `goose serve`. You can verify with:

```bash
lsof -nP -iTCP:3000 -sTCP:LISTEN
```

The output should show the address as `*:3000` or the specific external IP, not `127.0.0.1:3000`.

### TLS is not enabled

In the server's startup logs:

- If you see `listening on http://...`, TLS is **not** enabled. goose Desktop can still connect over HTTP, but this is not recommended for remote servers. Start with `--tls` or `GOOSE_TLS=true` and restart `goose serve`.
- If you see `listening on https://...`, TLS is enabled and you are good to go.

The startup logs also contain the `GOOSED_CERT_FINGERPRINT=...` line you can use for certificate pinning in goose Desktop. Search the server's stdout (or log file, if running under `launchd`) for `GOOSED_CERT_FINGERPRINT` to find it.

### Client cannot authenticate (401 / Unauthorized)

A `401` from the server, or a goose Desktop error indicating that the secret was rejected, almost always means that `GOOSE_SERVER__SECRET_KEY` on the server does not match the **Secret Key** in goose Desktop's settings.

To check the secret end-to-end without involving goose Desktop, run the authenticated `curl` from [step 2](#2-verify-the-server-is-up) using exactly the value you have configured on the client. For this `GET /acp` probe, a `406` response means authentication passed but the request did not include the SSE headers needed by the ACP stream. A `401` or `403` means the secret on the server is different from what you are sending.

If you rotate the secret on the server, you must also update it in goose Desktop's settings — they are not synchronized automatically.

### Certificate fingerprint mismatch

If goose Desktop refuses to connect with a certificate or fingerprint error, the most common causes are:

- The server regenerated its certificate (for example, after deleting the cert file). Look at the latest startup logs for the current `GOOSED_CERT_FINGERPRINT` and update goose Desktop.
- You copied the fingerprint with extra whitespace or pasted the wrong value.

## Related

- [Environment Variables](/docs/guides/environment-variables) — full reference for all `GOOSE_*` variables
- [Configuration Files](/docs/guides/config-files) — persistent client-side configuration
