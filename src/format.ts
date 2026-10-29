// Formatting of sensor readings for the UI. Pure functions, tested in format.test.ts.

export function formatValue(value: number | null | undefined, unit: string, digits = 0): string {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return "—";
  }
  return `${value.toFixed(digits)} ${unit}`;
}

export function percentOf(used: number | null | undefined, total: number | null | undefined): number | null {
  if (used == null || total == null || total <= 0) {
    return null;
  }
  return Math.min(100, Math.max(0, (used / total) * 100));
}
