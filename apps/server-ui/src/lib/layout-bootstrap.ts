function isPublicAnnouncerRoute(pathname: string): boolean {
  return pathname === "/announcer" || pathname.startsWith("/announcer/");
}

export function shouldBootstrapDashboard(pathname: string): boolean {
  return !isPublicAnnouncerRoute(pathname);
}
