"use client";

// Flick design-system components for the panel. Markup and class names follow the design
// system's `window.Flick` bundle (styles: flick-components.css), ported to React 19.
import Link from "next/link";
import {
  type ButtonHTMLAttributes,
  type ReactNode,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";

import { Icon, type IconName } from "./icons";

const cx = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join(" ");

// ---------------------------------------------------------------- Button

export type ButtonVariant = "primary" | "glass" | "ghost" | "danger";
export type ButtonSize = "sm" | "md" | "lg" | "icon-sm" | "icon" | "icon-lg";

export function Button({
  variant = "glass",
  size = "md",
  icon,
  label,
  className,
  children,
  ...rest
}: {
  variant?: ButtonVariant;
  size?: ButtonSize;
  icon?: IconName;
  /** Accessible name; required for icon-only sizes. */
  label?: string;
} & ButtonHTMLAttributes<HTMLButtonElement>) {
  const iconOnly = size.startsWith("icon");
  return (
    <button
      type="button"
      aria-label={label}
      title={iconOnly ? label : undefined}
      className={cx(
        "fk-btn fk-lift",
        `fk-btn--${size}`,
        `fk-btn--${variant}`,
        (variant === "glass" || variant === "danger") && "fk-glass",
        className,
      )}
      {...rest}
    >
      {icon && <Icon name={icon} />}
      {children}
    </button>
  );
}

/** A link that looks like a Button. */
export function ButtonLink({
  href,
  variant = "glass",
  size = "md",
  icon,
  children,
}: {
  href: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  icon?: IconName;
  children: ReactNode;
}) {
  return (
    <Link
      href={href}
      className={cx(
        "fk-btn fk-lift",
        `fk-btn--${size}`,
        `fk-btn--${variant}`,
        (variant === "glass" || variant === "danger") && "fk-glass",
      )}
    >
      {icon && <Icon name={icon} />}
      {children}
    </Link>
  );
}

// ---------------------------------------------------------------- Pill, Notice, Spinner

export function Pill({
  tone = "plain",
  children,
}: {
  tone?: "plain" | "strong" | "warn";
  children: ReactNode;
}) {
  return <span className={cx("fk-pill", `fk-pill--${tone}`)}>{children}</span>;
}

const NOTICE_ICON = { info: "info", warn: "triangle-alert", error: "circle-alert" } as const;

export function Notice({
  tone = "info",
  children,
}: {
  tone?: "info" | "warn" | "error";
  children: ReactNode;
}) {
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={cx("fk-notice fk-glass", `fk-notice--${tone}`)}
    >
      <Icon name={NOTICE_ICON[tone]} />
      <div className="fk-notice__body">{children}</div>
    </div>
  );
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div role="status" className="fk-spinner">
      <svg viewBox="0 0 24 24" aria-hidden="true">
        {Array.from({ length: 8 }, (_, i) => (
          <line
            key={i}
            x1={12}
            y1={3}
            x2={12}
            y2={7}
            stroke="currentColor"
            strokeWidth={2.2}
            strokeLinecap="round"
            opacity={0.25 + (i / 8) * 0.75}
            transform={`rotate(${i * 45} 12 12)`}
          />
        ))}
      </svg>
      {label && <span>{label}</span>}
    </div>
  );
}

// ---------------------------------------------------------------- Page scaffolding

export function PageHeader({
  title,
  lead,
  actions,
}: {
  title: ReactNode;
  lead?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="fk-pagehead">
      <div className="fk-pagehead__text">
        <h1 className="fk-pagehead__title">{title}</h1>
        {lead && <p className="fk-pagehead__lead">{lead}</p>}
      </div>
      {actions && <div className="fk-pagehead__actions">{actions}</div>}
    </header>
  );
}

export function Panel({
  title,
  actions,
  className,
  children,
}: {
  title?: ReactNode;
  actions?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section className={cx("fk-panel fk-glass", className)}>
      {(title || actions) && (
        <div className="fk-panel__head">
          {title && <h2 className="fk-panel__title">{title}</h2>}
          {actions}
        </div>
      )}
      {children}
    </section>
  );
}

export function Facts({ rows, mono }: { rows: [ReactNode, ReactNode][]; mono?: boolean }) {
  return (
    <dl className={cx("fk-facts", mono && "fk-facts--mono")}>
      {rows.map(([k, v], i) => (
        <div key={i} style={{ display: "contents" }}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}

export function SettingsGroup({
  title,
  note,
  children,
}: {
  title?: string;
  note?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="fk-group">
      {title && <h2 className="fk-group__title">{title}</h2>}
      <div className="fk-group__box fk-glass">{children}</div>
      {note && <div className="fk-group__note">{note}</div>}
    </section>
  );
}

export function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="fk-info">
      <span className="fk-info__label">{label}</span>
      <span className="fk-info__value">{children}</span>
    </div>
  );
}

export function EmptyState({
  title,
  children,
  actions,
}: {
  title: string;
  children?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="fk-empty fk-empty--inline">
      <h2 className="fk-empty__title">{title}</h2>
      {children && <div className="fk-empty__body">{children}</div>}
      {actions && <div className="fk-empty__actions">{actions}</div>}
    </div>
  );
}

// ---------------------------------------------------------------- Segmented

export function Segmented<T extends string>({
  options,
  value,
  onChange,
  label,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
  label: string;
}) {
  return (
    <div role="tablist" aria-label={label} className="fk-seg fk-glass fk-seg--sm">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="tab"
          aria-selected={o.value === value}
          className="fk-seg__item fk-lift"
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------- Switch

/** On / off switch (the design system's `.fk-switch`). */
export function Switch({
  id,
  checked,
  onChange,
  label,
  disabled,
}: {
  id?: string;
  checked: boolean;
  onChange: (on: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      className="fk-switch"
      onClick={() => onChange(!checked)}
    >
      <span className="fk-switch__thumb" />
    </button>
  );
}

// ---------------------------------------------------------------- TextField

export function TextField({
  label,
  value,
  onChange,
  type = "text",
  name,
  autoFocus,
  autoComplete = "off",
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  type?: "text" | "password";
  name?: string;
  autoFocus?: boolean;
  autoComplete?: string;
}) {
  return (
    <label className="fk-field">
      <span className="fk-field__label">{label}</span>
      <input
        className="fk-field__input fk-glass"
        type={type}
        name={name}
        value={value}
        autoFocus={autoFocus}
        autoComplete={autoComplete}
        spellCheck={false}
        onChange={(e) => onChange(e.currentTarget.value)}
      />
    </label>
  );
}

// ---------------------------------------------------------------- Dialog

/** Modal card on the dimmed scrim. Escape and the Close button dismiss; focus returns on close. */
export function Dialog({
  title,
  description,
  onClose,
  children,
}: {
  title: string;
  description?: ReactNode;
  onClose: () => void;
  children?: ReactNode;
}) {
  const id = useId();
  const card = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    card.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      previous?.focus?.();
    };
  }, [onClose]);

  return (
    <div
      className="fk-overlay fk-overlay--fixed"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={card}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={id}
        className="fk-dialog fk-glass-strong"
      >
        <header className="fk-dialog__head">
          <div className="fk-dialog__text">
            <h2 id={id} className="fk-dialog__title">
              {title}
            </h2>
            {description && <p className="fk-dialog__desc">{description}</p>}
          </div>
          <Button
            variant="ghost"
            size="icon"
            icon="x"
            label="Close"
            onClick={onClose}
            className="fk-dialog__close"
          />
        </header>
        {children}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- Copy button

/** Copies `text`, then reads "Copied" for two seconds. */
export function CopyButton({
  text,
  children,
  variant = "primary",
  size = "md",
}: {
  text: string;
  children: ReactNode;
  variant?: ButtonVariant;
  size?: ButtonSize;
}) {
  const [done, setDone] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!done && !failed) return;
    const t = setTimeout(() => {
      setDone(false);
      setFailed(false);
    }, 2000);
    return () => clearTimeout(t);
  }, [done, failed]);

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setDone(true);
    } catch {
      // Clipboard API needs HTTPS or localhost; fall back to a temporary textarea.
      try {
        const area = document.createElement("textarea");
        area.value = text;
        area.style.position = "fixed";
        area.style.opacity = "0";
        document.body.appendChild(area);
        area.select();
        const ok = document.execCommand("copy");
        area.remove();
        if (ok) setDone(true);
        else setFailed(true);
      } catch {
        setFailed(true);
      }
    }
  }

  return (
    <Button variant={variant} size={size} icon={done ? "check" : "copy"} onClick={copy}>
      {done ? "Copied" : failed ? "Copy Failed" : children}
    </Button>
  );
}
