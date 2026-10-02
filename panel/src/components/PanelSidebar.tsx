"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";

import { logout } from "@/app/login/actions";
import { HOME, MODULES } from "@/lib/modules";

import { Icon, Logo } from "./flick/icons";

const STORAGE_KEY = "flick-panel-sidebar-collapsed";

/** Flick's floating glass sidebar: modules on top, Collapse and Sign Out at the bottom. */
export function PanelSidebar() {
  const pathname = usePathname();
  const [collapsed, setCollapsed] = useState(false);

  // Remembered per device, like the app. Read after mount to keep server and client markup equal.
  useEffect(() => {
    try {
      setCollapsed(localStorage.getItem(STORAGE_KEY) === "1");
    } catch {
      /* storage unavailable: stay expanded */
    }
  }, []);

  function toggle() {
    setCollapsed((c) => {
      try {
        localStorage.setItem(STORAGE_KEY, c ? "0" : "1");
      } catch {
        /* ignore */
      }
      return !c;
    });
  }

  const items = [HOME, ...MODULES];
  const active = (href: string) => (href === "/" ? pathname === "/" : pathname.startsWith(href));

  return (
    <aside className={`fk-sidebar${collapsed ? " fk-sidebar--collapsed" : ""}`}>
      <div className="fk-sidebar__panel fk-glass">
        <div className="fk-sidebar__brand">
          <span className="fk-sidebar__clip">
            <Logo variant="wordmark" height="1.2rem" title="Flick" />
          </span>
        </div>
        <nav aria-label="Main" className="fk-sidebar__nav">
          {items.map((it) => (
            <Link
              key={it.id}
              href={it.href}
              className="fk-navitem fk-lift"
              aria-current={active(it.href) ? "page" : undefined}
              title={collapsed ? it.label : undefined}
            >
              <Icon name={it.icon} />
              <span className="fk-navitem__label">{it.label}</span>
            </Link>
          ))}
        </nav>
        <div className="fk-sidebar__foot">
          <button
            type="button"
            className="fk-navitem fk-navitem--button fk-lift"
            onClick={toggle}
            aria-label={collapsed ? "Expand Sidebar" : "Collapse Sidebar"}
            title={collapsed ? "Expand Sidebar" : undefined}
          >
            <Icon name={collapsed ? "panel-left-open" : "panel-left-close"} />
            <span className="fk-navitem__label">{collapsed ? "Expand Sidebar" : "Collapse Sidebar"}</span>
          </button>
          <form action={logout}>
            <button
              type="submit"
              className="fk-navitem fk-navitem--button fk-lift"
              aria-label="Sign Out"
              title={collapsed ? "Sign Out" : undefined}
            >
              <Icon name="log-out" />
              <span className="fk-navitem__label">Sign Out</span>
            </button>
          </form>
        </div>
      </div>
    </aside>
  );
}
