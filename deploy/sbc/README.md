# SBC Deployment Guide

## Overview

These files help deploy the rt-forwarder to a Raspberry Pi. The deployment
uses a two-phase approach: **cloud-init** provisions the OS baseline (hostname,
SSH keys, system user, required packages), and then an **interactive setup
script** (`rt-setup.sh`) downloads the forwarder binary, collects configuration
values, and installs the systemd service.

## Prerequisites

- Raspberry Pi 3, 4, or 5 running a 64-bit OS
- An SD card (16 GB+ recommended)
- [Raspberry Pi Imager](https://www.raspberrypi.com/software/) 2.0 or later
- A computer on the same network as the Pi
- A forwarder auth token from the server operator

## Step 1 -- Flash the SD Card

1. Open Raspberry Pi Imager.
2. Choose **Raspberry Pi OS Lite (64-bit)** as the operating system. The "Lite"
   variant is recommended because the forwarder runs headless -- no desktop
   environment is needed.
3. Select your SD card as the target storage device.
4. Click **Write** and wait for the flash to complete.

## Step 2 -- Configure cloud-init

You can configure these files either manually or with the helper wizard.

### Option A -- Generate files with the helper wizard (recommended)

From the repository root:

```bash
uv run scripts/sbc_cloud_init.py
```

The script asks for hostname, SSH key, static IP settings, and DNS servers, then
writes ready-to-copy `user-data` and `network-config` files.

To enable fully automatic first boot (no SSH setup commands), use:

```bash
uv run scripts/sbc_cloud_init.py --auto-first-boot
```

This mode also asks for forwarder setup values (server URL, token, reader
targets), then embeds a one-time non-interactive `rt-setup.sh` run in
`user-data`.

> **Security note:** `--auto-first-boot` stores the forwarder token in cloud-init
> data on the SD card. Use a scoped per-device token and rotate/revoke as needed.

### Option B -- Edit files manually

1. Open `deploy/sbc/user-data.yaml` from this repository in a text editor.

2. Change the two lines marked **CHANGEME**:

   - **`hostname`** -- set a unique name for this device (e.g. `rt-fwd-01`,
     `rt-fwd-02`).
   - **`ssh_authorized_keys`** -- replace the placeholder with your SSH public
     key. You can find it with:

     ```bash
     cat ~/.ssh/id_ed25519.pub
     # or
     cat ~/.ssh/id_rsa.pub
     ```

3. Open `deploy/sbc/network-config` and edit the static IP settings:

   - **`addresses`** -- the static IP for this Pi (default: `192.168.1.50/24`).
   - **`routes` → `via`** -- the default gateway (default: `192.168.1.1`).
   - **`nameservers`** -- DNS servers (default: `8.8.8.8`, `8.8.4.4`).

4. Copy both files to the SD card's **boot** partition:

   - `user-data.yaml` → `user-data` (no extension)
   - `network-config` → `network-config` (no extension)

> **Tip:** Some versions of Raspberry Pi Imager can apply cloud-init settings
> directly in the UI -- check under the advanced/customization options.

## Step 3 -- Boot and Connect

If you used `--auto-first-boot`, boot the Pi and wait 2--3 minutes. The
forwarder install/config is applied automatically via cloud-init on first boot.
SSH is optional for troubleshooting only.

1. Insert the SD card into the Pi and power it on.
2. Wait approximately **2 minutes** for the first boot and cloud-init to finish.
3. Connect via SSH using the static IP you configured in `network-config`:

   ```bash
   ssh pi@<static-ip-from-network-config>
   ```

   For example, if you kept the default address:

   ```bash
   ssh pi@192.168.1.50
   ```

   You can also try mDNS if your network supports it:

   ```bash
   ssh pi@<hostname>.local
   ```

## Step 4 -- Run the Setup Script

If you used `--auto-first-boot`, skip this step. `rt-setup.sh` already ran
automatically during first boot.

You have two options:

### Option A -- Download and run directly

```bash
curl -fsSL https://raw.githubusercontent.com/iwismer/rusty-timer/master/deploy/sbc/rt-setup.sh -o rt-setup.sh
sudo bash rt-setup.sh
```

### Option B -- If you cloned the repo

```bash
sudo bash deploy/sbc/rt-setup.sh
```

The setup script downloads both the release archive and its `.sha256` file,
then verifies the checksum before installing.

The wizard will prompt you for:

| Prompt | Example | Notes |
|---|---|---|
| Server base URL | `https://timing.example.com` | Must start with `http://` or `https://` |
| Auth token | *(hidden input)* | Provided by the server operator |
| Reader target(s) | `192.168.1.100:10000` | IP:PORT of each IPICO reader; enter one per line, blank line to finish |
| Status HTTP bind address | `0.0.0.0:8080` | Press Enter to accept the default |

## Step 5 -- Verify

The setup script runs verification automatically after installation. If you
choose not to restart an already-running service, the script skips verification
and prints follow-up commands to run after restart.

You can also check manually at any time:

```bash
# Check the service is running
sudo systemctl status rt-forwarder

# Hit the health endpoint
curl http://localhost:8080/healthz

# Follow logs in real time
journalctl -u rt-forwarder -f
```

## Updating the Forwarder

To update to a newer version, choose one of:

- **Re-run the setup script.** Answer **yes** when asked to re-download the
  binary, and **no** when asked to overwrite the existing configuration.

  ```bash
  sudo bash rt-setup.sh
  ```

- **Manual update.** Download the new `forwarder-*-aarch64-unknown-linux-gnu.tar.gz` from
  [GitHub Releases](https://github.com/iwismer/rusty-timer/releases), extract
  it, copy the binary to `/usr/local/bin/rt-forwarder`, and restart the service:

  ```bash
  sudo systemctl restart rt-forwarder
  ```

When the forwarder self-updater stages an artifact at
`/var/lib/rusty-timer/.forwarder-staged`, `systemd` applies it automatically on
the next restart via `/usr/local/lib/rt-forwarder-apply-staged.sh`.

On SBC installs, `POST /update/apply` is configured to restart the forwarder
process (instead of in-process binary replacement). The root-owned
`ExecStartPre` hook then atomically promotes the staged binary before startup.

## Configuration Reference

See [`docs/runbooks/forwarder-operations.md`](../../docs/runbooks/forwarder-operations.md)
for full configuration options and operational procedures.

## Troubleshooting

| Problem | Cause | Solution |
|---|---|---|
| Can't SSH into Pi | cloud-init still running, or wrong hostname | Wait 2--3 minutes after boot. Try the IP address instead of the hostname. |
| Setup script fails: "missing required commands" | One or more required tools are missing (`curl`, `jq`, `tar`, `sha256sum`) | Run `sudo apt-get install -y curl jq tar coreutils` |
| Setup script fails to download binary | No internet access on Pi | Check the network connection. Ensure the Pi can reach the internet. |
| Forwarder won't start | Bad config or unreachable readers | Check logs: `journalctl -u rt-forwarder -n 50` |
| "permission denied" errors | Script not running as root | Run with `sudo bash rt-setup.sh` |
| Forwarder starts but no events reach server | Wrong server URL or auth token | Verify `server.base_url` in `/etc/rusty-timer/forwarder.toml` and check the token in `/etc/rusty-timer/forwarder.token`. |
| Can't reach Pi after setting static IP | Wrong subnet or IP conflict | Verify the IP/subnet in `network-config` matches your network. Check for IP conflicts. Connect a monitor to see boot logs. |
