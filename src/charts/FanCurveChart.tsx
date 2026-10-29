// The fan curve editor's chart: plain SVG (D9 keeps uPlot for the charts over time; a curve with
// draggable points needs neither its time axis nor its canvas). Colours are CSS classes on the
// tokens of App.css, so the chart follows light and dark without reading the palette.
//
// Every point is a focusable slider: Left/Right move its temperature, Up/Down its speed (Shift
// for bigger steps), Delete removes it when the GPU lets the point count change. Dragging and
// the keys both go through `movePoint`, which keeps the curve inside the GPU's rules.

import { type KeyboardEvent, type PointerEvent, useRef, useState } from "react";
import { BIG_STEP, canEditPointCount, movePoint, percent, removePoint, speedAt, ticks } from "../fan.ts";
import { formatValue } from "../format.ts";
import type { CurvePoint, CurveRules } from "../tuning.ts";

type Props = {
  curve: CurvePoint[];
  rules: CurveRules;
  disabled: boolean;
  onChange: (curve: CurvePoint[]) => void;
  /** The temperature the curve follows, now; null when the sensor is missing. */
  temperature: number | null;
  /** The speed of the fan now, in RPM; null when the GPU does not report it. */
  rpm: number | null;
};

const WIDTH = 600;
const HEIGHT = 260;
const MARGIN = { left: 48, right: 16, top: 16, bottom: 32 };
const POINT_RADIUS = 7;
const TICK_COUNT = 6;

export function FanCurveChart({ curve, rules, disabled, onChange, temperature, rpm }: Props) {
  const svg = useRef<SVGSVGElement>(null);
  const [dragging, setDragging] = useState<number | null>(null);
  const { min_c, max_c } = rules.temperature;
  const span = Math.max(1, max_c - min_c);
  const plotW = WIDTH - MARGIN.left - MARGIN.right;
  const plotH = HEIGHT - MARGIN.top - MARGIN.bottom;
  const x = (t: number) => MARGIN.left + ((t - min_c) / span) * plotW;
  const y = (s: number) => MARGIN.top + (1 - s / 100) * plotH;

  const move = (index: number, target: CurvePoint) => onChange(movePoint(curve, index, target, rules));

  const toCurve = (e: PointerEvent<SVGCircleElement>): CurvePoint | null => {
    const matrix = svg.current?.getScreenCTM();
    if (!matrix) return null;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(matrix.inverse());
    return {
      temperature_c: min_c + ((p.x - MARGIN.left) / plotW) * span,
      speed_pct: (1 - (p.y - MARGIN.top) / plotH) * 100,
    };
  };

  const onKeyDown = (index: number, e: KeyboardEvent<SVGCircleElement>) => {
    if (disabled) return;
    const point = curve[index];
    const step = e.shiftKey ? BIG_STEP : 1;
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowDown: [0, -step],
      ArrowUp: [0, step],
    };
    const delta = moves[e.key];
    if (delta !== undefined) {
      e.preventDefault();
      move(index, { temperature_c: point.temperature_c + delta[0], speed_pct: point.speed_pct + delta[1] });
    } else if ((e.key === "Delete" || e.key === "Backspace") && canEditPointCount(rules)) {
      const next = removePoint(curve, index, rules);
      if (next !== null) {
        e.preventDefault();
        onChange(next);
      }
    }
  };

  const first = curve[0];
  const last = curve[curve.length - 1];
  // Flat past the ends, as the fan follows it.
  const line =
    first && last
      ? [
          `${x(min_c)},${y(first.speed_pct)}`,
          ...curve.map((p) => `${x(p.temperature_c)},${y(p.speed_pct)}`),
          `${x(max_c)},${y(last.speed_pct)}`,
        ].join(" ")
      : "";
  const now = temperature !== null && temperature >= min_c && temperature <= max_c ? temperature : null;
  const target = now !== null ? speedAt(curve, now) : null;

  return (
    <figure className="fan-curve">
      <svg
        ref={svg}
        className="fan-curve-plot"
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        role="group"
        aria-label="Fan curve: temperature against fan speed"
      >
        {/* Speeds the GPU does not accept. */}
        <rect
          className="fan-curve-out"
          x={MARGIN.left}
          y={y(100)}
          width={plotW}
          height={Math.max(0, y(rules.speed.max_pct) - y(100))}
        />
        <rect
          className="fan-curve-out"
          x={MARGIN.left}
          y={y(rules.speed.min_pct)}
          width={plotW}
          height={Math.max(0, y(0) - y(rules.speed.min_pct))}
        />
        {ticks(min_c, max_c, TICK_COUNT).map((t) => (
          <g key={`x${t}`}>
            <line className="fan-curve-grid" x1={x(t)} x2={x(t)} y1={MARGIN.top} y2={MARGIN.top + plotH} />
            <text className="fan-curve-axis" x={x(t)} y={HEIGHT - MARGIN.bottom / 3} textAnchor="middle">
              {`${t} °C`}
            </text>
          </g>
        ))}
        {ticks(0, 100, 4).map((s) => (
          <g key={`y${s}`}>
            <line className="fan-curve-grid" x1={MARGIN.left} x2={WIDTH - MARGIN.right} y1={y(s)} y2={y(s)} />
            <text className="fan-curve-axis" x={MARGIN.left - 6} y={y(s)} textAnchor="end" dominantBaseline="middle">
              {`${s} %`}
            </text>
          </g>
        ))}
        {now !== null && (
          <g className="fan-curve-now">
            <line x1={x(now)} x2={x(now)} y1={MARGIN.top} y2={MARGIN.top + plotH} />
            {target !== null && <circle cx={x(now)} cy={y(target)} r={POINT_RADIUS / 2} />}
          </g>
        )}
        <polyline className="fan-curve-line" points={line} />
        {curve.map((p, i) => (
          <circle
            // The order of the points never changes while editing: the index is stable.
            key={i}
            className="fan-curve-point"
            data-dragging={dragging === i || undefined}
            cx={x(p.temperature_c)}
            cy={y(p.speed_pct)}
            r={POINT_RADIUS}
            tabIndex={disabled ? -1 : 0}
            role="slider"
            aria-label={`Point ${i + 1}`}
            aria-valuemin={rules.speed.min_pct}
            aria-valuemax={rules.speed.max_pct}
            aria-valuenow={p.speed_pct}
            aria-valuetext={`${p.temperature_c} °C, ${percent(p.speed_pct)}`}
            aria-disabled={disabled || undefined}
            onKeyDown={(e) => onKeyDown(i, e)}
            onPointerDown={(e) => {
              if (disabled) return;
              e.currentTarget.setPointerCapture(e.pointerId);
              e.currentTarget.focus();
              setDragging(i);
            }}
            onPointerMove={(e) => {
              if (dragging !== i) return;
              const target = toCurve(e);
              if (target !== null) move(i, target);
            }}
            onPointerUp={() => setDragging(null)}
            onPointerCancel={() => setDragging(null)}
          />
        ))}
      </svg>
      <figcaption className="fan-curve-caption tuning-number">
        Now {formatValue(temperature, "°C")} · {formatValue(rpm, "RPM")}
        {target !== null && <> · curve asks for {percent(Math.round(target))}</>}
      </figcaption>
    </figure>
  );
}
