import { buttonClass, inputMonoClass } from "@rusty-timer/shared-ui";

// Receiver inputs are monospaced (host:port, node IDs, ticket strings).
export const inputClass = inputMonoClass;

export const btnPrimary = buttonClass("primary");

export const btnSecondary = buttonClass("secondary");

// Deliberately different from the shared "danger" variant: uses the softer
// status-err-border/status-err-bg treatment for the disconnect affordance.
export const btnDisconnect =
  "px-3 py-1.5 text-sm font-medium rounded-md text-status-err border border-status-err-border bg-status-err-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";
