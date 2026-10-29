// The chart colours, read from the tokens in App.css. A canvas cannot use CSS variables, so the
// values are read with getComputedStyle and read again whenever the theme can have changed:
// the system colour scheme, or an attribute on the root element (a `data-theme` set by the app).

import { useEffect, useState } from "react";

export type ChartPalette = {
  primary: string;
  secondary: string;
  axis: string;
  grid: string;
  /** Canvas font for the axes, from --text-sm and the root font family. */
  font: string;
  /** Height of the plot area in CSS pixels, from --chart-height. */
  height: number;
};

const FALLBACK_HEIGHT = 180;

function readPalette(): ChartPalette {
  const style = getComputedStyle(document.documentElement);
  // The body's family: it reflects a declaration on <body> and inherits one made on :root.
  const fontFamily = getComputedStyle(document.body).fontFamily;
  const token = (name: string) => style.getPropertyValue(name).trim();
  const height = Number.parseFloat(token("--chart-height"));
  return {
    primary: token("--chart-primary"),
    secondary: token("--chart-secondary"),
    axis: token("--muted"),
    grid: token("--chart-grid"),
    font: `${token("--text-sm")} ${fontFamily}`,
    height: Number.isFinite(height) && height > 0 ? height : FALLBACK_HEIGHT,
  };
}

function samePalette(a: ChartPalette, b: ChartPalette): boolean {
  return (Object.keys(a) as (keyof ChartPalette)[]).every((key) => a[key] === b[key]);
}

/** The current palette; a new object only when a value actually changed. */
export function useChartPalette(): ChartPalette {
  const [palette, setPalette] = useState(readPalette);

  useEffect(() => {
    const update = () =>
      setPalette((previous) => {
        const next = readPalette();
        return samePalette(previous, next) ? previous : next;
      });
    const scheme = window.matchMedia("(prefers-color-scheme: dark)");
    scheme.addEventListener("change", update);
    const observer = new MutationObserver(update);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class", "style"] });
    update();
    return () => {
      scheme.removeEventListener("change", update);
      observer.disconnect();
    };
  }, []);

  return palette;
}
