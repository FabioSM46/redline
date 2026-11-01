import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { Loaded } from "./RecorderContext.tsx";
import { indicatorLabel, indicatorVersion, type LastCheck, type UpdateStatus, updateRow } from "./updates.ts";

const LOADING: Loaded<UpdateStatus> = { status: "loading" };
const UNREADABLE: Loaded<UpdateStatus> = { status: "error", message: "no state" };
const FAILURES: LastCheck[] = ["curl_missing", "unreachable", "refused", "bad_response"];
const OUTCOMES: LastCheck[] = ["never", "ok", ...FAILURES];

const base: UpdateStatus = {
  current_version: "0.2.1",
  latest_version: null,
  update_available: false,
  dismissed: false,
  last_check: "never",
};

function ready(change: Partial<UpdateStatus> = {}): Loaded<UpdateStatus> {
  return { status: "ready", value: { ...base, ...change } };
}

const available = { latest_version: "0.3.0", update_available: true };

describe("status bar indicator", () => {
  it("names the version of an undismissed update, whatever the last check did", () => {
    for (const last_check of OUTCOMES) {
      assert.equal(indicatorVersion(ready({ ...available, last_check })), "0.3.0", last_check);
    }
    assert.equal(indicatorLabel("0.3.0"), "Update 0.3.0 available");
  });

  it("shows nothing for a dismissed version", () => {
    assert.equal(indicatorVersion(ready({ ...available, dismissed: true, last_check: "ok" })), null);
  });

  it("stays silent without an update, while loading and on every failure", () => {
    assert.equal(indicatorVersion(LOADING), null);
    assert.equal(indicatorVersion(UNREADABLE), null);
    for (const last_check of OUTCOMES) {
      assert.equal(indicatorVersion(ready({ last_check })), null, last_check);
    }
    assert.equal(indicatorVersion(ready({ latest_version: "0.2.1", last_check: "ok" })), null);
  });
});

describe("General row", () => {
  it("says it is reading until the status is known, whatever the setting", () => {
    for (const enabled of [true, false, null]) {
      assert.deepEqual(updateRow(LOADING, enabled), {
        installed: null,
        available: null,
        canDismiss: false,
        message: "Reading the update status…",
      });
    }
  });

  it("reports an unreadable status with what to do", () => {
    for (const enabled of [true, false, null]) {
      const row = updateRow(UNREADABLE, enabled);
      assert.equal(row.installed, null);
      assert.equal(row.message, "Cannot read the update status: no state. Restart Redline to try again.");
    }
  });

  it("reads the disabled state from the setting before any never sentence", () => {
    assert.deepEqual(updateRow(ready(), false), {
      installed: "0.2.1",
      available: null,
      canDismiss: false,
      message: "Update checks are off.",
    });
  });

  it("offers nothing while the check is off, even from a status not yet reset", () => {
    for (const last_check of OUTCOMES) {
      for (const dismissed of [false, true]) {
        const row = updateRow(ready({ ...available, dismissed, last_check }), false);
        assert.equal(row.available, null);
        assert.equal(row.canDismiss, false);
        assert.equal(row.message, "Update checks are off.");
      }
    }
  });

  it("promises a check only when the setting is known to be on", () => {
    assert.equal(updateRow(ready(), true).message, "No check has finished yet. The first one runs shortly.");
    assert.equal(updateRow(ready(), null).message, null);
  });

  it("says the installed version is current after a good check", () => {
    const row = updateRow(ready({ latest_version: "0.2.1", last_check: "ok" }), true);
    assert.deepEqual(row, {
      installed: "0.2.1",
      available: null,
      canDismiss: false,
      message: "Redline is up to date.",
    });
  });

  it("does not call a release older than the build available", () => {
    const row = updateRow(ready({ current_version: "0.4.0", latest_version: "0.3.0", last_check: "ok" }), true);
    assert.equal(row.available, null);
    assert.equal(row.message, "Redline is up to date.");
  });

  it("names the available version and lets it be dismissed", () => {
    assert.deepEqual(updateRow(ready({ ...available, last_check: "ok" }), true), {
      installed: "0.2.1",
      available: "0.3.0",
      canDismiss: true,
      message: "Version 0.3.0 is available.",
    });
  });

  it("keeps the row after a dismissal, which it cannot repeat", () => {
    assert.deepEqual(updateRow(ready({ ...available, dismissed: true, last_check: "ok" }), true), {
      installed: "0.2.1",
      available: "0.3.0",
      canDismiss: false,
      message: "Version 0.3.0 is available. Dismissed: the status bar shows only a later version.",
    });
  });

  it("gives every failed check its own sentence with what to do", () => {
    const expected: [LastCheck, RegExp][] = [
      ["curl_missing", /Install curl/],
      ["unreachable", /Check the network/],
      ["refused", /request limit.*tries again/],
      ["bad_response", /could not be read.*tries again/],
    ];
    for (const [last_check, sentence] of expected) {
      const row = updateRow(ready({ last_check }), true);
      assert.equal(sentence.test(row.message ?? ""), true, last_check);
      assert.equal(row.available, null);
      assert.equal(row.installed, "0.2.1");
    }
  });

  it("keeps a known update next to a later failure", () => {
    for (const last_check of FAILURES) {
      const row = updateRow(ready({ ...available, last_check }), true);
      assert.equal(row.available, "0.3.0");
      assert.equal(row.canDismiss, true);
      assert.equal(/^Version 0\.3\.0 is available\. \S/.test(row.message ?? ""), true, last_check);
    }
  });

  it("does not need the setting to report what a check found", () => {
    assert.equal(updateRow(ready({ ...available, last_check: "ok" }), null).available, "0.3.0");
    const failed = updateRow(ready({ last_check: "unreachable" }), null);
    assert.equal(failed.message?.startsWith("GitHub could not be reached."), true);
  });
});
