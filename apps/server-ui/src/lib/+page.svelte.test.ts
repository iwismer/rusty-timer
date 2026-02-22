import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/svelte";
import Page from "../routes/+page.svelte";

describe("root streams page", () => {
  it("renders the streams heading", () => {
    render(Page);

    expect(screen.getByTestId("streams-heading")).toHaveTextContent("Streams");
  });
});
