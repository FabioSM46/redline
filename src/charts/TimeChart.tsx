// A small React wrapper around uPlot. It owns the chart's lifetime: the instance is created
// once, fed new data through `setData`, resized with its container and destroyed on unmount.
// It is rebuilt only when the series layout (`chart`) or the palette (a theme change) changes,
// never because the parent re-rendered.

import { useEffect, useRef } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { formatValue } from "../format.ts";
import { type ChartSpec, type Columns, formatClock, type SeriesRole, yRange } from "../metrics.ts";
import type { ChartPalette } from "./useChartPalette.ts";

type Props = {
  chart: ChartSpec;
  data: Columns;
  /** The x range in seconds (the whole window); the newest sample sits at the right edge. */
  range: [number, number];
  /** Axis labels show seconds only on the shortest window. */
  showSeconds: boolean;
  palette: ChartPalette;
};

const LINE_WIDTH = 1.5;
const LIMIT_DASH = [6, 4];
const X_LABEL_SPACE = 80;
const Y_AXIS_SIZE = 72;

export function TimeChart({ chart, data, range, showSeconds, palette }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  // The values the chart reads while it draws; kept current without rebuilding it.
  const latest = useRef({ data, range, showSeconds });

  // Declared before the creation effect so a new instance always starts from the latest data.
  useEffect(() => {
    latest.current = { data, range, showSeconds };
    plot.current?.setData(data);
  }, [data, range, showSeconds]);

  useEffect(() => {
    const element = host.current;
    if (!element) return;

    const stroke = (role: SeriesRole) => (role === "primary" ? palette.primary : palette.secondary);
    const axis = {
      stroke: palette.axis,
      font: palette.font,
      grid: { stroke: palette.grid, width: 1 },
      ticks: { stroke: palette.grid, width: 1 },
    };
    const options: uPlot.Options = {
      width: element.clientWidth,
      height: palette.height,
      cursor: { drag: { x: false, y: false } },
      scales: {
        x: { time: true, range: () => latest.current.range },
        y: { range: (_u, _min, max) => yRange(chart, Number.isFinite(max) ? max : null) },
      },
      axes: [
        {
          ...axis,
          space: X_LABEL_SPACE,
          values: (_u, splits) => splits.map((s) => formatClock(s, latest.current.showSeconds)),
        },
        {
          ...axis,
          size: Y_AXIS_SIZE,
          values: (_u, splits) => splits.map((v) => `${v} ${chart.unit}`),
        },
      ],
      series: [
        { label: "Time", value: (_u, v) => (v == null ? "—" : formatClock(v, true)) },
        ...chart.series.map((s) => ({
          label: s.label,
          stroke: stroke(s.role),
          width: LINE_WIDTH,
          dash: s.role === "limit" ? LIMIT_DASH : undefined,
          spanGaps: false,
          value: (_u: uPlot, v: number | null) => formatValue(v, chart.unit),
        })),
      ],
    };

    const instance = new uPlot(options, latest.current.data, element);
    plot.current = instance;

    const resize = new ResizeObserver(() => {
      if (element.clientWidth > 0) instance.setSize({ width: element.clientWidth, height: palette.height });
    });
    resize.observe(element);

    return () => {
      resize.disconnect();
      instance.destroy();
      plot.current = null;
    };
  }, [chart, palette]);

  return <div className="chart-plot" ref={host} />;
}
