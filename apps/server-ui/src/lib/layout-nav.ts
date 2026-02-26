export type LayoutNavLink = {
  href: string;
  label: string;
  active: boolean;
};

function isPublicAnnouncerRoute(pathname: string): boolean {
  return pathname === "/announcer" || pathname.startsWith("/announcer/");
}

export function getLayoutNavLinks(pathname: string): LayoutNavLink[] {
  if (isPublicAnnouncerRoute(pathname)) {
    return [];
  }

  return [
    {
      href: "/",
      label: "Streams",
      active:
        pathname === "/" ||
        pathname.startsWith("/streams") ||
        pathname.startsWith("/forwarders"),
    },
    {
      href: "/races",
      label: "Races",
      active: pathname.startsWith("/races"),
    },
    {
      href: "/announcer-config",
      label: "Announcer",
      active: pathname.startsWith("/announcer-config"),
    },
    {
      href: "/logs",
      label: "Logs",
      active: pathname.startsWith("/logs"),
    },
    {
      href: "/admin",
      label: "Admin",
      active: pathname.startsWith("/admin"),
    },
  ];
}
