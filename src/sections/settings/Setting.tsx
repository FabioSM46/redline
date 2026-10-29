import type { ReactNode } from "react";

// The row and state pieces shared by the Settings pages.

/** A value read from the backend: loading, unreadable, or known. */
export type Loaded<T> = { status: "loading" } | { status: "error"; message: string } | { status: "ready"; value: T };

/** What a control is doing after a change: nothing, saving, or the reason the last save failed. */
export type SaveState = { status: "idle" } | { status: "saving" } | { status: "error"; message: string };

export const IDLE: SaveState = { status: "idle" };

export function Setting({
  title,
  description,
  save,
  note,
  wide = false,
  children,
}: {
  title: string;
  description: string;
  save: SaveState;
  /** What follows from the last saved change, if anything. */
  note?: string | null;
  /** The control below the text, full width (lists, text fields), instead of on the right. */
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <div className={wide ? "setting setting-wide" : "setting"}>
      <div className="setting-text">
        <span className="setting-title">{title}</span>
        <span className="setting-description">{description}</span>
        {save.status === "error" && (
          <span className="setting-error" role="alert">
            Not saved: {save.message}
          </span>
        )}
        {note && (
          <span className="setting-note" role="status">
            {note}
          </span>
        )}
      </div>
      <div className="setting-control">{children}</div>
    </div>
  );
}

/** The control once its value is known; a short state line while loading or when unreadable. */
export function Value<T>({ state, what, children }: { state: Loaded<T>; what: string; children: (value: T) => ReactNode }) {
  if (state.status === "loading") return <span className="setting-description">Loading…</span>;
  if (state.status === "error") {
    return (
      <span className="setting-error" role="alert">
        Cannot read {what}: {state.message}
      </span>
    );
  }
  return <>{children(state.value)}</>;
}
