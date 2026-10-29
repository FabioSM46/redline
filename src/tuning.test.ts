import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  canApply,
  canReset,
  changeLabel,
  clampWatts,
  formatCountdown,
  lactUnreachable,
  NO_PENDING,
  outcomeSentence,
  overdriveView,
  PENDING_EVENT,
  type PendingChange,
  pendingReducer,
  planExpired,
  type PowerLimit,
  powerRange,
  secondsLeft,
  SETTLED_EVENT,
  stillPending,
  TUNING_CHANGES,
  type TuningStatus,
} from "./tuning.ts";

const power: PowerLimit = { current_w: 250, min_w: 150, max_w: 300, default_w: 250 };

const status: TuningStatus = {
  available: true,
  unavailable_reason: null,
  warning: null,
  lact_version: "0.10.1",
  overdrive_enabled: false,
  reboot_pending: false,
  power,
  fan: null,
  profile: null,
  pending: null,
};

const change: PendingChange = { gpu_id: "0000:03:00.0", what: "power_limit", deadline_ms: 10_000 };
const other: PendingChange = { gpu_id: "0000:0c:00.0", what: "power_limit", deadline_ms: 20_000 };

describe("powerRange", () => {
  it("needs both ends", () => {
    assert.deepEqual(powerRange(power), { min: 150, max: 300 });
    assert.equal(powerRange({ ...power, min_w: null }), null);
    assert.equal(powerRange({ ...power, max_w: null }), null);
    assert.equal(powerRange(null), null);
  });

  it("refuses an inverted range", () => {
    assert.equal(powerRange({ ...power, min_w: 310 }), null);
  });
});

describe("clampWatts", () => {
  const range = { min: 150, max: 300 };

  it("rounds to whole watts and keeps the value in range", () => {
    assert.equal(clampWatts(212.4, range), 212);
    assert.equal(clampWatts(212.6, range), 213);
    assert.equal(clampWatts(90, range), 150);
    assert.equal(clampWatts(400, range), 300);
  });

  it("is null for something that is not a number", () => {
    assert.equal(clampWatts(Number.NaN, range), null);
    assert.equal(clampWatts(Number.POSITIVE_INFINITY, range), null);
  });
});

describe("canApply", () => {
  it("is true for a different value in range", () => {
    assert.equal(canApply(200, power), true);
  });

  it("is false for the limit already in force, out of range or without a range", () => {
    assert.equal(canApply(250, power), false);
    assert.equal(canApply(100, power), false);
    assert.equal(canApply(null, power), false);
    assert.equal(canApply(200, { ...power, max_w: null }), false);
  });

  it("allows a value when the current limit is unknown", () => {
    assert.equal(canApply(250, { ...power, current_w: null }), true);
  });
});

describe("canReset", () => {
  it("is false at the default and true away from it or when either is unknown", () => {
    assert.equal(canReset(power), false);
    assert.equal(canReset({ ...power, current_w: 200 }), true);
    assert.equal(canReset({ ...power, default_w: null }), true);
    assert.equal(canReset(null), false);
  });
});

describe("countdown", () => {
  it("rounds up and stops at zero", () => {
    assert.equal(secondsLeft(10_000, 5_000), 5);
    assert.equal(secondsLeft(10_000, 5_001), 5);
    assert.equal(secondsLeft(10_000, 9_999), 1);
    assert.equal(secondsLeft(10_000, 10_000), 0);
    assert.equal(secondsLeft(10_000, 12_000), 0);
    assert.equal(formatCountdown(4), "4 s");
  });
});

describe("pendingReducer", () => {
  it("records a pending change once, from the event or the reply", () => {
    const state = pendingReducer(NO_PENDING, { type: "pending", change });
    assert.deepEqual(state.pending, change);
    assert.equal(pendingReducer(state, { type: "pending", change: { ...change } }), state);
  });

  it("settles the pending change once and says how", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change });
    const settled = pendingReducer(pending, { type: "settled", result: { gpu_id: change.gpu_id, outcome: "expired" } });
    assert.equal(settled.pending, null);
    assert.deepEqual(settled.settled, { change, result: { gpu_id: change.gpu_id, outcome: "expired" } });
    assert.equal(pendingReducer(settled, { type: "settled", result: { gpu_id: change.gpu_id, outcome: "reverted" } }), settled);
  });

  it("ignores a settle for another GPU", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change });
    assert.equal(pendingReducer(pending, { type: "settled", result: { gpu_id: other.gpu_id, outcome: "confirmed" } }), pending);
  });

  it("never brings back a settled change from a late event or a stale status", () => {
    const settled = pendingReducer(pendingReducer(NO_PENDING, { type: "pending", change }), {
      type: "settled",
      result: { gpu_id: change.gpu_id, outcome: "confirmed" },
    });
    assert.equal(pendingReducer(settled, { type: "pending", change }), settled);
    assert.equal(pendingReducer(settled, { type: "status", change, nowMs: 0 }), settled);
  });

  it("takes a change from the status only while its deadline is ahead", () => {
    assert.deepEqual(pendingReducer(NO_PENDING, { type: "status", change, nowMs: 9_000 }).pending, change);
    assert.equal(pendingReducer(NO_PENDING, { type: "status", change, nowMs: 10_000 }), NO_PENDING);
  });

  it("keeps the change it already holds over a status reply", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change: other });
    assert.equal(pendingReducer(pending, { type: "status", change, nowMs: 0 }), pending);
  });

  it("settles from the answer of confirm or revert when the event is missed", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change });
    const result = { gpu_id: change.gpu_id, outcome: "confirmed" as const };
    assert.deepEqual(pendingReducer(pending, { type: "answered", change, result }), { pending: null, settled: { change, result } });
    assert.equal(pendingReducer(pending, { type: "answered", change: other, result }), pending);
  });

  it("a new change replaces the outcome of the previous one", () => {
    const settled = pendingReducer(pendingReducer(NO_PENDING, { type: "pending", change }), {
      type: "settled",
      result: { gpu_id: change.gpu_id, outcome: "reverted" },
    });
    assert.deepEqual(pendingReducer(settled, { type: "pending", change: other }), { pending: other, settled: null });
  });
});

describe("sentences", () => {
  it("names the change and every outcome", () => {
    assert.equal(changeLabel("power_limit"), "Power limit");
    assert.equal(changeLabel("fan"), "Fan");
    assert.equal(changeLabel("clocks" as never), "Tuning change");
    for (const outcome of ["confirmed", "reverted", "expired"] as const) {
      assert.equal(outcomeSentence({ gpu_id: "x", outcome }).length > 0, true);
    }
  });
});

describe("status views", () => {
  it("sends to Dependencies only when LACT cannot be reached", () => {
    assert.equal(lactUnreachable({ ...status, available: false, lact_version: null }), true);
    assert.equal(lactUnreachable({ ...status, available: false }), false);
    assert.equal(lactUnreachable(status), false);
  });

  it("puts a pending reboot before the running kernel's bit", () => {
    assert.equal(overdriveView(status), "disabled");
    assert.equal(overdriveView({ ...status, reboot_pending: true }), "reboot");
    assert.equal(overdriveView({ ...status, overdrive_enabled: true }), "enabled");
    assert.equal(overdriveView({ ...status, overdrive_enabled: null }), "unknown");
  });

  it("an overdrive plan expires at its deadline", () => {
    const plan = { action: "enable" as const, steps: [], reboot_required: true, token: "t", expires_ms: 1_000 };
    assert.equal(planExpired(plan, 999), false);
    assert.equal(planExpired(plan, 1_000), true);
  });
});

describe("mirrors of src-tauri/src/tuning.rs", () => {
  const rust = readFileSync(new URL("../src-tauri/src/tuning.rs", import.meta.url), "utf8");
  const overdrive = readFileSync(new URL("../src-tauri/src/tuning/overdrive.rs", import.meta.url), "utf8");

  it("uses the same event names", () => {
    assert.equal(rust.includes(`PENDING_EVENT: &str = "${PENDING_EVENT}"`), true);
    assert.equal(rust.includes(`SETTLED_EVENT: &str = "${SETTLED_EVENT}"`), true);
  });

  it("has every field of the serialized structs", () => {
    const fields = (source: string, name: string) => {
      const body = source.slice(source.indexOf(`pub struct ${name} {`));
      return [...body.slice(0, body.indexOf("\n}")).matchAll(/^\s+pub (\w+):/gm)].map((m) => m[1]).sort();
    };
    assert.deepEqual(fields(rust, "TuningStatus"), Object.keys(status).sort());
    assert.deepEqual(fields(rust, "PowerLimit"), Object.keys(power).sort());
    assert.deepEqual(fields(rust, "PendingChange"), Object.keys(change).sort());
    assert.deepEqual(fields(rust, "Settled"), ["gpu_id", "outcome"]);
    assert.deepEqual(fields(overdrive, "OverdrivePlan"), ["action", "expires_ms", "reboot_required", "steps", "token"]);
  });

  it("has a label for every kind of change", () => {
    const body = rust.slice(rust.indexOf("pub enum TuningChange {"));
    const variants = [...body.slice(0, body.indexOf("}")).matchAll(/^\s+(\w+),/gm)].map((m) =>
      m[1].replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase(),
    );
    assert.equal(variants.length > 0, true);
    for (const variant of variants) {
      assert.equal((TUNING_CHANGES as readonly string[]).includes(variant), true, variant);
    }
  });
});

describe("a missed tuning-settled", () => {
  it("ends the change as expired once the backend no longer holds it after the deadline", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change });
    assert.equal(pendingReducer(pending, { type: "gone", change, nowMs: 9_999 }), pending);
    assert.deepEqual(pendingReducer(pending, { type: "gone", change, nowMs: 10_000 }), {
      pending: null,
      settled: { change, result: { gpu_id: change.gpu_id, outcome: "expired" } },
    });
    assert.equal(pendingReducer(pending, { type: "gone", change: other, nowMs: 30_000 }), pending);
  });

  it("reads whether the status still holds the change", () => {
    assert.equal(stillPending({ pending: change }, change), true);
    assert.equal(stillPending({ pending: null }, change), false);
    assert.equal(stillPending({ pending: { ...change, deadline_ms: 11_000 } }, change), false);
  });
});
