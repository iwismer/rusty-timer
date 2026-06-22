import { describe, expect, it } from "vitest";

import layoutSource from "../routes/+layout.svelte?raw";
import navBarSource from "../../../shared-ui/src/components/NavBar.svelte?raw";

describe("server announcer navigation", () => {
  it("adds Announcer Feed as a right-side NavBar action", () => {
    expect(layoutSource).toContain("actions={[");
    expect(layoutSource).toContain('href: "/announcer"');
    expect(layoutSource).toContain('label: "Announcer Feed"');
    expect(navBarSource).toContain("actions = []");
  });
});
