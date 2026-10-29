import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  applyInstallEvent,
  type Dependency,
  dependencyAdvice,
  DEPENDENCY_IDS,
  DEPENDENCY_INSTALL_EVENT,
  dependencyState,
  INSTALL_IDLE,
  installLine,
  installRefused,
  installStarted,
  LACT_SOCKET_FALLBACK,
  parseDependencies,
  REDLINE_LICENSE,
  THIRD_PARTY_LICENSES,
  versionLine,
} from "./dependencies.ts";
import { SETTINGS_TABS } from "./tabs.ts";

const DEPENDENCIES_RS = readFileSync(new URL("../src-tauri/src/dependencies.rs", import.meta.url), "utf8");
const INSTALLER_RS = readFileSync(new URL("../src-tauri/src/installer.rs", import.meta.url), "utf8");
const PACKAGE_JSON = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));

/** The variants of `pub enum <name>` in a Rust source, as serde snake_case names. */
function rustEnum(source: string, name: string): string[] {
  const start = source.indexOf(`pub enum ${name} {`);
  assert.equal(start >= 0, true, `enum ${name} not found`);
  const body = source.slice(source.indexOf("{", start) + 1, source.indexOf("}", start));
  return [...body.matchAll(/^\s*(\w+),?\s*$/gm)].map((m) => m[1].replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase());
}

/** The field names of `pub struct <name>` in a Rust source. */
function rustFields(source: string, name: string): string[] {
  const start = source.indexOf(`pub struct ${name} {`);
  assert.equal(start >= 0, true, `struct ${name} not found`);
  const body = source.slice(start, source.indexOf("\n}", start));
  return [...body.matchAll(/^\s*pub (\w+):/gm)].map((m) => m[1]);
}

function dep(overrides: Partial<Dependency> = {}): Dependency {
  return {
    id: "ffmpeg",
    name: "ffmpeg",
    installed: true,
    version: "7.0",
    verified_version: null,
    warning: null,
    hint: null,
    daemon: null,
    can_install: true,
    ...overrides,
  };
}

describe("mirror of the backend contract", () => {
  it("has the same dependency ids, in the same order", () => {
    assert.deepEqual([...DEPENDENCY_IDS], rustEnum(DEPENDENCIES_RS, "DependencyId"));
  });

  it("has the same Dependency and DaemonStatus fields", () => {
    assert.deepEqual(Object.keys(dep()).sort(), rustFields(DEPENDENCIES_RS, "Dependency").sort());
    assert.deepEqual(["running", "socket_accessible"], rustFields(DEPENDENCIES_RS, "DaemonStatus").sort());
  });

  it("has the same event name and install stages", () => {
    assert.equal(INSTALLER_RS.includes(`pub const INSTALL_EVENT: &str = "${DEPENDENCY_INSTALL_EVENT}";`), true);
    assert.deepEqual(rustEnum(INSTALLER_RS, "InstallStage"), ["progress", "succeeded", "failed", "cancelled"]);
    assert.deepEqual(rustFields(INSTALLER_RS, "InstallEvent"), ["id", "stage", "message"]);
  });

  it("names Redline's own license as package.json does", () => {
    assert.equal(REDLINE_LICENSE, PACKAGE_JSON.license);
  });
});

describe("Settings tabs", () => {
  it("include Dependencies and About, last", () => {
    assert.deepEqual(
      SETTINGS_TABS.map((t) => t.id).slice(-2),
      ["dependencies", "about"],
    );
  });
});

describe("parseDependencies", () => {
  it("keeps known entries and reads missing fields as absent", () => {
    const parsed = parseDependencies([
      { id: "lact", name: "LACT", installed: true, daemon: { running: true } },
      { id: "unknown", name: "X", installed: true },
      null,
    ]);
    assert.equal(parsed.length, 1);
    assert.deepEqual(parsed[0], {
      id: "lact",
      name: "LACT",
      installed: true,
      version: null,
      verified_version: null,
      warning: null,
      hint: null,
      daemon: { running: true, socket_accessible: false },
      can_install: false,
    });
  });

  it("never reads a missing installed flag as installed", () => {
    assert.equal(parseDependencies([{ id: "ffmpeg" }])[0].installed, false);
  });

  it("answers an empty list for anything that is not a list", () => {
    assert.deepEqual(parseDependencies(null), []);
    assert.deepEqual(parseDependencies({}), []);
  });
});

describe("dependencyState and versionLine", () => {
  it("tells installed, missing and unreachable apart", () => {
    assert.equal(dependencyState(dep()).kind, "installed");
    assert.equal(dependencyState(dep({ installed: false })).kind, "missing");
    const lact = dep({ id: "lact", daemon: { running: true, socket_accessible: false } });
    assert.deepEqual(dependencyState(lact), { kind: "attention", label: "Not reachable" });
    assert.equal(dependencyState(dep({ warning: "unverified" })).kind, "attention");
  });

  it("shows the installed version against the verified one", () => {
    assert.equal(versionLine(dep({ version: "0.9.0", verified_version: "0.10.1" })), "Version 0.9.0, verified with 0.10.1");
    assert.equal(versionLine(dep({ version: null })), "Version unknown");
    assert.equal(versionLine(dep({ installed: false, version: null, verified_version: "6.1.6" })), "Version none, verified with 6.1.6");
  });
});

describe("dependencyAdvice", () => {
  it("prefers the backend's hint", () => {
    assert.equal(dependencyAdvice(dep({ hint: "Start lactd." })), "Start lactd.");
  });

  it("explains the admin group when lactd refuses the user and there is no hint", () => {
    const lact = dep({ id: "lact", daemon: { running: true, socket_accessible: false } });
    assert.equal(dependencyAdvice(lact), LACT_SOCKET_FALLBACK);
    assert.equal(LACT_SOCKET_FALLBACK.includes("admin group"), true);
  });

  it("says nothing when nothing is wrong", () => {
    assert.equal(dependencyAdvice(dep()), null);
  });
});

describe("install state", () => {
  it("follows progress to success", () => {
    let state = installStarted("lact");
    assert.deepEqual(installLine(state, "lact"), { text: "Installing… waiting for the password", error: false });
    state = applyInstallEvent(state, { id: "lact", stage: "progress", message: "Downloading LACT" });
    assert.deepEqual(state, { status: "running", id: "lact", step: "Downloading LACT" });
    state = applyInstallEvent(state, { id: "lact", stage: "progress", message: null });
    assert.deepEqual(state, { status: "running", id: "lact", step: "Downloading LACT" });
    state = applyInstallEvent(state, { id: "lact", stage: "succeeded", message: null });
    assert.deepEqual(state, { status: "succeeded", id: "lact" });
    // The command's final result repeats the last event.
    assert.deepEqual(applyInstallEvent(state, { id: "lact", stage: "succeeded", message: null }), state);
  });

  it("shows a closed password prompt as cancelled, not as an error", () => {
    const state = applyInstallEvent(installStarted("ffmpeg"), { id: "ffmpeg", stage: "cancelled", message: null });
    assert.equal(installLine(state, "ffmpeg")?.error, false);
    assert.equal((installLine(state, "ffmpeg")?.text ?? "").includes("cancelled"), true);
  });

  it("keeps the helper's message on failure", () => {
    const state = applyInstallEvent(installStarted("lact"), {
      id: "lact",
      stage: "failed",
      message: "LACT is not supported on this system.",
    });
    assert.deepEqual(installLine(state, "lact"), { text: "Install failed: LACT is not supported on this system.", error: true });
  });

  it("shows a refusal before anything ran as a failure", () => {
    const state = installRefused("ffmpeg", "Another install is running: wait for it to finish.");
    assert.deepEqual(installLine(state, "ffmpeg"), {
      text: "Install failed: Another install is running: wait for it to finish.",
      error: true,
    });
  });

  it("shows the outcome only on its own row and ignores payloads it does not know", () => {
    const state = installStarted("lact");
    assert.equal(installLine(state, "ffmpeg"), null);
    assert.equal(installLine(INSTALL_IDLE, "lact"), null);
    assert.equal(applyInstallEvent(state, { id: "x", stage: "succeeded" }), state);
    assert.equal(applyInstallEvent(state, { id: "lact", stage: "done" }), state);
    assert.equal(applyInstallEvent(state, "nonsense"), state);
  });

  it("picks up an install started before the page opened", () => {
    const state = applyInstallEvent(INSTALL_IDLE, { id: "ffmpeg", stage: "progress", message: "Installing ffmpeg" });
    assert.deepEqual(state, { status: "running", id: "ffmpeg", step: "Installing ffmpeg" });
  });
});

describe("third-party licenses", () => {
  it("name LACT (MIT) and GPU Screen Recorder (GPL-3.0) with https addresses", () => {
    assert.deepEqual(
      THIRD_PARTY_LICENSES.map((t) => [t.name, t.license]),
      [
        ["LACT", "MIT"],
        ["GPU Screen Recorder", "GPL-3.0"],
      ],
    );
    for (const t of THIRD_PARTY_LICENSES) assert.equal(t.url.startsWith("https://"), true);
  });
});
