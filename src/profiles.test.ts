import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  canSwitch,
  checkName,
  deleteBlocker,
  MAX_NAME_CHARS,
  PROFILE_EVENT,
  type Profiles,
  profileFromValue,
  profileLabel,
  profilesReducer,
  profileValue,
  switchDetail,
} from "./profiles.ts";

const profiles: Profiles = { profiles: ["Gaming", "Quiet"], active: "Quiet", auto_switch: false };

const refused = (input: string, existing: string[] = []) => {
  const check = checkName(input, existing);
  assert.equal(check.ok, false, input);
  return check.ok ? "" : check.message;
};

describe("checkName", () => {
  it("trims the name and accepts a new one", () => {
    assert.deepEqual(checkName("  Silent  ", profiles.profiles), { ok: true, name: "Silent" });
  });

  it("refuses an empty name", () => {
    assert.equal(refused(""), "A profile needs a name.");
    assert.equal(refused("   "), "A profile needs a name.");
  });

  it("counts characters, not UTF-16 units", () => {
    assert.equal(checkName("é".repeat(MAX_NAME_CHARS), []).ok, true);
    assert.equal(checkName("🎮".repeat(MAX_NAME_CHARS), []).ok, true);
    assert.equal(refused("x".repeat(MAX_NAME_CHARS + 1)), "A profile name can be at most 64 characters long.");
  });

  it("refuses control characters", () => {
    assert.equal(refused("a\nb"), "A profile name cannot contain line breaks or other control characters.");
    assert.equal(refused("a\u0007b").includes("control characters"), true);
  });

  it("refuses LACT's default profile name in any case", () => {
    assert.equal(refused("default").startsWith('"Default" is the name'), true);
    assert.equal(refused(" DeFaUlT ").startsWith('"Default" is the name'), true);
    assert.equal(checkName("Default 2", []).ok, true);
  });

  it("refuses a name that exists, ignoring case", () => {
    assert.equal(refused("quiet", profiles.profiles), 'A profile named "quiet" already exists.');
    assert.equal(refused("GAMING", profiles.profiles), 'A profile named "GAMING" already exists.');
  });
});

describe("select values", () => {
  it("round-trips the default profile and named ones", () => {
    assert.equal(profileFromValue(profileValue(null)), null);
    assert.equal(profileFromValue(profileValue("Quiet")), "Quiet");
  });

  it("labels the default profile", () => {
    assert.equal(profileLabel(null), "Default");
    assert.equal(switchDetail(null, "Gaming"), "Default → Gaming");
    assert.equal(switchDetail("Gaming", null), "Gaming → Default");
  });
});

describe("canSwitch", () => {
  it("allows another existing profile or the default one", () => {
    assert.equal(canSwitch(profiles, "Gaming"), true);
    assert.equal(canSwitch(profiles, null), true);
  });

  it("refuses the active profile, an unknown one and any switch while LACT switches by itself", () => {
    assert.equal(canSwitch(profiles, "Quiet"), false);
    assert.equal(canSwitch(profiles, "Gone"), false);
    assert.equal(canSwitch({ ...profiles, auto_switch: true }, "Gaming"), false);
    assert.equal(canSwitch({ ...profiles, active: null }, null), false);
  });
});

describe("deleteBlocker", () => {
  it("lets an inactive profile go", () => {
    assert.equal(deleteBlocker(profiles, "Gaming"), null);
  });

  it("keeps the active profile and the default one", () => {
    assert.equal(deleteBlocker(profiles, "Quiet")?.includes("switch to another profile"), true);
    assert.equal(deleteBlocker(profiles, null)?.includes("cannot be deleted"), true);
  });
});

describe("profilesReducer", () => {
  const other: Profiles = { ...profiles, active: "Gaming" };

  it("takes every event", () => {
    const state = profilesReducer({ events: 0, value: null }, { type: "event", profiles });
    assert.deepEqual(state, { events: 1, value: profiles });
  });

  it("drops a reply requested before the last event", () => {
    const afterEvent = profilesReducer({ events: 0, value: null }, { type: "event", profiles: other });
    assert.equal(profilesReducer(afterEvent, { type: "reply", profiles, since: 0 }), afterEvent);
    assert.deepEqual(profilesReducer(afterEvent, { type: "reply", profiles, since: 1 }), {
      events: 1,
      value: profiles,
    });
  });
});

describe("mirrors of src-tauri/src/tuning/profiles.rs", () => {
  // Rust's `\` line continuation drops the line break and the indentation that follows it.
  const rust = readFileSync(new URL("../src-tauri/src/tuning/profiles.rs", import.meta.url), "utf8").replace(
    /\\\n\s*/g,
    "",
  );

  it("uses the same event name and name limit", () => {
    assert.equal(rust.includes(`PROFILE_EVENT: &str = "${PROFILE_EVENT}"`), true);
    assert.equal(rust.includes(`MAX_NAME_CHARS: usize = ${MAX_NAME_CHARS};`), true);
  });

  it("has every field of Profiles", () => {
    const body = rust.slice(rust.indexOf("pub struct Profiles {"));
    const fields = [...body.slice(0, body.indexOf("\n}")).matchAll(/^\s+pub (\w+):/gm)].map((m) => m[1]).sort();
    assert.deepEqual(fields, Object.keys(profiles).sort());
  });

  it("says the same sentences as check_name", () => {
    for (const input of ["", "a\nb", "default"]) {
      const sentence = refused(input).replace(/"/g, '\\"');
      assert.equal(rust.includes(`"${sentence}"`), true, sentence);
    }
    assert.equal(rust.includes('"A profile named \\"{name}\\" already exists."'), true);
    assert.equal(rust.includes('"A profile name can be at most {MAX_NAME_CHARS} characters long."'), true);
    assert.equal(
      rust.includes(`"${deleteBlocker(profiles, "Quiet")}"`),
      true,
      "the active-profile sentence matches ACTIVE_PROFILE",
    );
  });
});
