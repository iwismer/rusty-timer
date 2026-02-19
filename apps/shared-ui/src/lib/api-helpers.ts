export async function apiFetch<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
  const resp = await fetch(path, {
    headers: { "Content-Type": "application/json", ...(init?.headers ?? {}) },
    ...init,
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(
      `API ${init?.method ?? "GET"} ${path} -> ${resp.status}: ${text}`,
    );
  }
  if (resp.status === 204) return undefined as unknown as T;
  return resp.json();
}
