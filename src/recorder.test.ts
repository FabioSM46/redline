import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  createThumbnailCache,
  formatClipTime,
  formatDuration,
  latestClips,
  type Recording,
  type RecorderStatus,
  replayIndicator,
  replayIsOn,
  THUMBNAIL_RETRY_MS,
  unavailableNotice,
} from "./recorder.ts";

const status: RecorderStatus = {
  available: true,
  unavailable_reason: null,
  replay: "off",
  recording: false,
  replay_seconds: 60,
  error: null,
  warning: null,
};

function clip(file_name: string, saved_at_ms: number): Recording {
  return { id: `replay:${file_name}`, kind: "replay", file_name, saved_at_ms, size_bytes: 1, duration_s: null };
}

describe("replay indicator", () => {
  it("names the length and the state", () => {
    assert.equal(replayIndicator({ ...status, replay: "on" }), "⏺ Replay 60 s · ON");
    assert.equal(replayIndicator({ ...status, replay: "off", replay_seconds: 30 }), "⏺ Replay 30 s · OFF");
    assert.equal(replayIndicator({ ...status, replay: "starting" }), "⏺ Replay 60 s · STARTING");
    assert.equal(replayIndicator({ ...status, replay: "error" }), "⏺ Replay 60 s · OFF");
  });

  it("is on while starting, so the switch stops a replay that is coming up", () => {
    assert.equal(replayIsOn({ ...status, replay: "on" }), true);
    assert.equal(replayIsOn({ ...status, replay: "starting" }), true);
    assert.equal(replayIsOn({ ...status, replay: "off" }), false);
    assert.equal(replayIsOn({ ...status, replay: "error" }), false);
  });
});

describe("unavailable notice", () => {
  it("is null when GPU Screen Recorder is there", () => {
    assert.equal(unavailableNotice(status), null);
  });

  it("says what to install when it is missing", () => {
    const reason = "Install GPU Screen Recorder.";
    assert.equal(unavailableNotice({ ...status, available: false, unavailable_reason: reason }), reason);
  });

  it("is a wait before the first detection", () => {
    assert.equal(unavailableNotice({ ...status, available: false }), "Looking for GPU Screen Recorder…");
  });
});

describe("latest clips", () => {
  it("keeps the newest five, newest first, ties by file name", () => {
    const clips = [clip("a", 1), clip("b", 7), clip("d", 5), clip("c", 5), clip("e", 3), clip("f", 9), clip("g", 2)];
    assert.deepEqual(
      latestClips(clips).map((c) => c.file_name),
      ["f", "b", "c", "d", "e"],
    );
  });

  it("honours a smaller limit and never mutates the input", () => {
    const clips = [clip("a", 1), clip("b", 2)];
    assert.deepEqual(
      latestClips(clips, 1).map((c) => c.file_name),
      ["b"],
    );
    assert.deepEqual(
      clips.map((c) => c.file_name),
      ["a", "b"],
    );
    assert.deepEqual(latestClips([], 3), []);
  });
});

describe("clip formatting", () => {
  it("shows the time for today and the date otherwise", () => {
    const now = new Date(2026, 9, 29, 18, 0).getTime();
    assert.equal(formatClipTime(new Date(2026, 9, 29, 9, 5).getTime(), now), "Today 09:05");
    assert.equal(formatClipTime(new Date(2026, 9, 28, 23, 59).getTime(), now), "2026-10-28 23:59");
  });

  it("writes durations as m:ss or h:mm:ss, and null when unknown", () => {
    assert.equal(formatDuration(42.4), "0:42");
    assert.equal(formatDuration(65), "1:05");
    assert.equal(formatDuration(3725), "1:02:05");
    assert.equal(formatDuration(null), null);
    assert.equal(formatDuration(Number.NaN), null);
  });
});

describe("thumbnail cache", () => {
  it("asks once per clip and shares a request between parallel callers", async () => {
    const asked: string[] = [];
    const cache = createThumbnailCache(async (id) => {
      asked.push(id);
      return `data:${id}`;
    });
    assert.equal(cache.peek("a"), undefined);
    const [one, two] = await Promise.all([cache.get("a"), cache.get("a")]);
    assert.equal(one, "data:a");
    assert.equal(two, "data:a");
    assert.equal(await cache.get("a"), "data:a");
    assert.equal(cache.peek("a"), "data:a");
    assert.deepEqual(asked, ["a"]);
  });

  it("asks again for a missing preview only after the retry delay", async () => {
    let clock = 0;
    let calls = 0;
    const cache = createThumbnailCache(async () => (++calls === 1 ? null : "data:late"), () => clock);
    assert.equal(await cache.get("a"), null);
    assert.equal(cache.peek("a"), null);
    clock = THUMBNAIL_RETRY_MS - 1;
    assert.equal(await cache.get("a"), null);
    clock = THUMBNAIL_RETRY_MS;
    assert.equal(await cache.get("a"), "data:late");
    assert.equal(calls, 2);
  });

  it("treats a failed request as no preview", async () => {
    const cache = createThumbnailCache(() => Promise.reject(new Error("gone")));
    assert.equal(await cache.get("a"), null);
  });

  it("forgets clips that left the list, including an answer still in flight", async () => {
    let release: (url: string) => void = () => {};
    const cache = createThumbnailCache(
      (id) => (id === "slow" ? new Promise<string>((resolve) => (release = resolve)) : Promise.resolve(`data:${id}`)),
    );
    await cache.get("a");
    const slow = cache.get("slow");
    cache.retain(["b"]);
    release("data:slow");
    assert.equal(await slow, "data:slow");
    assert.equal(cache.peek("a"), undefined);
    assert.equal(cache.peek("slow"), undefined);
  });
});
