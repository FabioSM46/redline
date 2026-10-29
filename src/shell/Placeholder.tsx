/** A short empty state for content that arrives in a later issue or milestone. */
export function Placeholder({ children }: { children: string }) {
  return <p className="state">{children}</p>;
}
