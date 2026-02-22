# Forwarder Power Actions: Path B Draft (No `sudo`, Keep `NoNewPrivileges=yes`)

## Goal

Enable `restart-device` and `shutdown-device` from the forwarder UI without calling `sudo`, while keeping the service hardening (`NoNewPrivileges=yes`) in place.

## Why Path B

Current behavior attempts this sequence:
1. `systemctl --no-ask-password reboot|poweroff`
2. fallback to `sudo -n systemctl ...`

This conflicts with hardening intent because `NoNewPrivileges=yes` is designed to prevent gaining new privileges via `execve()` (including setuid/setgid paths).

Path B removes `sudo` from the runtime path and authorizes only the exact reboot/shutdown operations through polkit.

## Proposed Architecture

1. Keep the forwarder service unit hardened (`NoNewPrivileges=yes`).
2. Remove `sudo` fallback from `services/forwarder/src/status_http.rs`.
3. Install a root-owned polkit rules file during SBC setup:
- `/etc/polkit-1/rules.d/90-rt-forwarder-power-actions.rules`
4. Restrict authorization to the `rt-forwarder` user and only power actions.
5. Tie setup-time policy install/remove to the persisted config (`[control].allow_power_actions`) to reduce drift at install/upgrade time.
6. Document post-install drift behavior: if `allow_power_actions` is toggled at runtime via config API, operators must re-run setup (or an explicit sync command) to reconcile polkit policy.

## Draft Polkit Rule

```javascript
// /etc/polkit-1/rules.d/90-rt-forwarder-power-actions.rules
polkit.addRule(function(action, subject) {
    if (subject.user !== "rt-forwarder") {
        return;
    }

    // logind power actions used by systemctl reboot/poweroff
    if (action.id === "org.freedesktop.login1.reboot" ||
        action.id === "org.freedesktop.login1.reboot-multiple-sessions") {
        return polkit.Result.YES;
    }
    if (action.id === "org.freedesktop.login1.power-off" ||
        action.id === "org.freedesktop.login1.power-off-multiple-sessions") {
        return polkit.Result.YES;
    }

    // scoped fallback if systemd manager authorization path is used
    if (action.id === "org.freedesktop.systemd1.manage-units") {
        var unit = action.lookup("unit");
        var verb = action.lookup("verb");
        if (verb === "start" && (unit === "reboot.target" || unit === "poweroff.target")) {
            return polkit.Result.YES;
        }
    }
});
```

Notes:
- This is intentionally narrow. It does not grant unit-file management or arbitrary unit control.
- It intentionally does not authorize `*-ignore-inhibit` actions.
- Rule file should be owned by `root:root` with mode `0644` (or stricter if distro policy requires).
- Setup should fail fast if `/etc/polkit-1/rules.d` is unavailable.

## Code Changes (Draft)

### `services/forwarder/src/status_http.rs`

1. Remove `run_power_action_command_with_sudo()`.
2. Simplify `run_power_action_command()` to one execution path:
- `systemctl --no-ask-password reboot|poweroff`
3. Keep `power_action_auth_failed()` mapping to `HTTP 403` for denied polkit auth.
4. Keep detailed stderr/stdout surfacing for operator diagnostics.

### `deploy/sbc/rt-setup.sh`

1. Add:
- `POWER_ACTIONS_POLKIT_RULES_PATH="/etc/polkit-1/rules.d/90-rt-forwarder-power-actions.rules"`
- `render_power_actions_polkit_rules()` helper
2. In `install_service()`:
- read persisted TOML value for `allow_power_actions` and treat that as source of truth
- if `allow_power_actions=true`: install polkit rules
- else: remove the polkit rules file if present
- always remove legacy sudoers file (`/etc/sudoers.d/90-rt-forwarder-power-actions`) during migration
3. Validate prerequisites only when `allow_power_actions=true`:
- ensure `polkitd` tooling/layout exists (`/etc/polkit-1/rules.d`)
- emit explicit warning/error if unsupported on the target image
4. Update console output so setup reports which policy was installed/removed.

### Docs

1. Replace sudoers guidance with polkit guidance in:
- `deploy/sbc/README.md`
- `docs/runbooks/forwarder-operations.md`
2. Add troubleshooting for polkit rule absence/misconfiguration.

## Security Properties

1. Preserves `NoNewPrivileges=yes` in the service unit.
2. Removes runtime dependency on setuid escalation (`sudo`).
3. Grants only reboot/poweroff actions to a single dedicated service user.
4. Supports least privilege better than broad `sudoers` rules.
5. Does not change the existing network trust model: control endpoints remain unauthenticated by design, so LAN isolation remains required.

## Migration Plan

1. Ship setup script capable of replacing old sudoers-based installs.
2. During install/upgrade:
- install polkit rules when persisted config has `allow_power_actions=true`
- delete legacy sudoers file
- restart forwarder
3. If `allow_power_actions` changes later through config editing, run a privileged sync step (or rerun setup) to update/remove the polkit rule.
4. Validate with:
- `POST /api/v1/control/restart-device`
- `POST /api/v1/control/shutdown-device`
- `GET /api/v1/logs` for denial/success diagnostics

## Test Plan

1. Shell helper tests:
- new renderer returns expected action IDs and user check
- install/remove gating follows persisted `allow_power_actions` value
- policy path, owner, and mode are asserted
- legacy sudoers removal is asserted
- behavior is covered when setup skips config overwrite
2. Forwarder tests:
- existing command error mapping tests remain green
- ensure auth failure still returns `403`
3. Manual SBC verification:
- positive path with rule installed
- negative path after removing rule (expect `403` + auth error details)

## Risks and Open Questions

1. Some distros may differ in polkit defaults or logind availability.
2. If `systemctl` authorization path differs, fallback coverage depends on `manage-units` details (`unit`/`verb`) being present as expected.
3. Need SBC smoke validation on target Raspberry Pi OS image before merge.
4. Path B does not solve unauthenticated control endpoints; it relies on trusted LAN boundaries. A follow-up hardening option is adding explicit auth on `/api/v1/control/*`.

## Research References

1. systemd `NoNewPrivileges=` semantics: https://man7.org/linux/man-pages/man5/systemd.exec.5.html
2. polkit JavaScript rules and `action.lookup(...)`: https://www.freedesktop.org/software/polkit/docs/latest/polkit.8.html
3. `login1` power/reboot polkit action IDs (upstream policy): https://raw.githubusercontent.com/systemd/systemd/main/src/login/org.freedesktop.login1.policy
4. `systemd1.manage-units` policy action: https://raw.githubusercontent.com/systemd/systemd/main/src/core/org.freedesktop.systemd1.policy.in
5. `manage-units` detail keys (`unit`, `verb`) in systemd source: https://raw.githubusercontent.com/systemd/systemd/main/src/core/dbus-util.c
6. `systemd1` D-Bus security model (manage-units requirement): https://raw.githubusercontent.com/systemd/systemd/main/man/org.freedesktop.systemd1.xml
