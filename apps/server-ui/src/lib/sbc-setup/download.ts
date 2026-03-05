// download.ts
export function downloadFile(filename: string, content: string): void {
  if (typeof document === "undefined")
    throw new Error("downloadFile requires a browser environment");
  const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  try {
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
  } finally {
    URL.revokeObjectURL(url);
  }
}
