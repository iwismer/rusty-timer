export type ForwarderConfigSection =
  | "general"
  | "server"
  | "auth"
  | "journal"
  | "uplink"
  | "status_http"
  | "readers";

export function getForwarderConfigSectionRows(): ForwarderConfigSection[][] {
  return [
    ["general", "server"],
    ["auth", "journal"],
    ["uplink", "status_http"],
    ["readers"],
  ];
}
