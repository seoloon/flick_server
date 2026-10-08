import Link from "next/link";

import { SCOPE_LABEL, SETTINGS_SCOPES } from "@/lib/settings-form";
import type { SettingsScope } from "@/lib/types";

/** Server / FlickSync / FlickDD, styled as the design system's segmented control. */
export function ScopeTabs({ current }: { current: SettingsScope }) {
  return (
    <nav aria-label="Settings scope" className="fk-seg fk-glass fk-seg--sm">
      {SETTINGS_SCOPES.map((s) => (
        <Link
          key={s}
          href={`/settings/${s}`}
          className="fk-seg__item fk-lift"
          aria-current={s === current ? "page" : undefined}
        >
          {SCOPE_LABEL[s]}
        </Link>
      ))}
    </nav>
  );
}
