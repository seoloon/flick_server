"use client";

import { type ReactNode, useId, useState } from "react";

import { Button, Pill, Switch } from "@/components/flick/ui";
import {
  type Edits,
  choiceOptions,
  fieldDisplay,
  fieldHint,
  fieldLabel,
  parseBool,
  resetEdit,
  setEdit,
  undoEdit,
} from "@/lib/settings-form";
import type { SettingField } from "@/lib/types";

const INPUT = "fk-field__input fk-glass setting__input";

/** One setting: label, variable name, where its value comes from, its control, Undo / Reset. */
export function FieldRow({
  field,
  edits,
  flagged,
  onEdit,
}: {
  field: SettingField;
  edits: Edits;
  /** Named in the last error message. */
  flagged: boolean;
  onEdit: (change: (edits: Edits) => Edits) => void;
}) {
  const id = useId();
  const [replacing, setReplacing] = useState(false);
  const d = fieldDisplay(field, edits);
  const label = fieldLabel(field.name);
  const set = (value: string) => onEdit((e) => setEdit(e, field, value));

  let control: ReactNode;
  if (field.kind === "bool") {
    control = (
      <Switch
        id={id}
        label={label}
        checked={parseBool(d.value) ?? false}
        disabled={d.removing}
        onChange={(on) => set(on ? "true" : "false")}
      />
    );
  } else if (field.kind === "choice") {
    control = (
      <select
        id={id}
        className={INPUT}
        value={d.value}
        disabled={d.removing}
        onChange={(e) => set(e.currentTarget.value)}
      >
        {choiceOptions(field, d.value).map((c) => (
          <option key={c} value={c}>
            {c}
          </option>
        ))}
      </select>
    );
  } else if (field.secret) {
    control =
      replacing || (d.changed && !d.removing) ? (
        <input
          id={id}
          className={INPUT}
          type="password"
          autoComplete="new-password"
          placeholder="New value"
          value={d.value}
          onChange={(e) => set(e.currentTarget.value)}
        />
      ) : (
        <>
          <Pill tone={field.set && !d.removing ? "strong" : "plain"}>
            {d.removing ? "Will be cleared" : field.set ? "Set" : "Not set"}
          </Pill>
          {!d.removing && (
            <Button size="sm" onClick={() => setReplacing(true)}>
              {field.set ? "Replace" : "Set Value"}
            </Button>
          )}
        </>
      );
  } else {
    control = (
      <input
        id={id}
        className={INPUT}
        type="text"
        inputMode={field.kind === "int" ? "numeric" : field.kind === "float" ? "decimal" : "text"}
        spellCheck={false}
        autoComplete="off"
        placeholder={field.kind === "list" ? "first, second" : undefined}
        value={d.value}
        disabled={d.removing}
        onChange={(e) => set(e.currentTarget.value)}
      />
    );
  }

  let trailing: ReactNode = null;
  if (d.changed) {
    trailing = (
      <Button
        variant="ghost"
        size="sm"
        onClick={() => {
          setReplacing(false);
          onEdit((e) => undoEdit(e, field.name));
        }}
      >
        Undo
      </Button>
    );
  } else if (replacing) {
    trailing = (
      <Button variant="ghost" size="sm" onClick={() => setReplacing(false)}>
        Cancel
      </Button>
    );
  } else if (field.source === "panel") {
    trailing = (
      <Button variant="ghost" size="sm" onClick={() => onEdit((e) => resetEdit(e, field))}>
        {field.secret ? "Clear" : "Reset"}
      </Button>
    );
  }

  return (
    <div className="setting" data-changed={d.changed ? "" : undefined} data-flagged={flagged ? "" : undefined}>
      <div className="setting__text">
        <label className="setting__label" htmlFor={id}>
          {label}
        </label>
        <span className="setting__name">{field.name}</span>
        {fieldHint(field, d) && <span className="setting__hint">{fieldHint(field, d)}</span>}
      </div>
      <div className="setting__control">
        {control}
        {trailing}
      </div>
    </div>
  );
}
