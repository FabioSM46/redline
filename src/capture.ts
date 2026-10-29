// Settings → Capture: the values the recording settings accept for codec, quality and frame
// rate, and the sentences shown for the screen choice.
//
// CODECS, QUALITIES and the frame rate range MIRROR `redline-core::recorder` (`CODECS`,
// `QUALITIES`, `FPS`, `PORTAL` in src-tauri/crates/redline-core/src/recorder.rs). The backend is
// the authority and refuses anything else; a change there must update this file too.
// `capture.test.ts` reads the Rust file and fails when the two lists drift apart.

/** The `recording` object of `get_settings` (`RecordingSettings` in src-tauri/src/settings.rs). */
export type RecordingSettings = {
  replay_seconds: number;
  audio_tracks: string[][];
  replay_dir: string;
  recording_dir: string;
  /** `portal`, or a monitor name such as `DP-2`. */
  capture: string;
  /** `null`: GPU Screen Recorder chooses. */
  codec: string | null;
  quality: string;
  fps: number;
  replay_on_start: boolean;
};

/** One choice of `list_capture_targets` (`Choice` in src-tauri/src/sources.rs). */
export type CaptureTarget = { id: string; label: string };

/** The capture value that goes through the desktop's screen sharing portal. */
export const PORTAL = "portal";

/** `redline-core::recorder::CODECS`, in the same order. */
export const CODECS = [
  "h264",
  "hevc",
  "av1",
  "vp8",
  "vp9",
  "hevc_hdr",
  "av1_hdr",
  "hevc_10bit",
  "av1_10bit",
  "h264_vulkan",
  "hevc_vulkan",
  "hevc_10bit_vulkan",
  "av1_vulkan",
  "av1_hdr_vulkan",
  "av1_10bit_vulkan",
] as const;

/** `redline-core::recorder::QUALITIES`, in the same order. */
export const QUALITIES = ["medium", "high", "very_high", "ultra"] as const;

/** `redline-core::recorder::FPS`. */
export const FPS_MIN = 1;
export const FPS_MAX = 500;

/** A group of codec choices; `null` is the automatic choice (no `-k` at all). */
export type CodecGroup = { label: string | null; options: { value: string | null; label: string }[] };

const CODEC_LABELS: Record<(typeof CODECS)[number], string> = {
  h264: "H.264",
  hevc: "HEVC",
  av1: "AV1",
  vp8: "VP8",
  vp9: "VP9",
  hevc_hdr: "HEVC HDR",
  av1_hdr: "AV1 HDR",
  hevc_10bit: "HEVC 10-bit",
  av1_10bit: "AV1 10-bit",
  h264_vulkan: "H.264",
  hevc_vulkan: "HEVC",
  hevc_10bit_vulkan: "HEVC 10-bit",
  av1_vulkan: "AV1",
  av1_hdr_vulkan: "AV1 HDR",
  av1_10bit_vulkan: "AV1 10-bit",
};

function codecGroup(label: string | null, values: readonly (typeof CODECS)[number][]): CodecGroup {
  return { label, options: values.map((value) => ({ value, label: CODEC_LABELS[value] })) };
}

/** Automatic first, then the common codecs; HDR, 10-bit and the experimental Vulkan ones apart. */
export const CODEC_GROUPS: readonly CodecGroup[] = [
  {
    label: null,
    options: [{ value: null, label: "Automatic" }, ...codecGroup(null, ["h264", "hevc", "av1", "vp8", "vp9"]).options],
  },
  codecGroup("HDR and 10-bit", ["hevc_hdr", "av1_hdr", "hevc_10bit", "av1_10bit"]),
  codecGroup("Vulkan (experimental)", [
    "h264_vulkan",
    "hevc_vulkan",
    "hevc_10bit_vulkan",
    "av1_vulkan",
    "av1_hdr_vulkan",
    "av1_10bit_vulkan",
  ]),
];

const QUALITY_LABELS: Record<(typeof QUALITIES)[number], string> = {
  medium: "Medium",
  high: "High",
  very_high: "Very high",
  ultra: "Ultra",
};

export const QUALITY_OPTIONS: readonly { value: string; label: string }[] = QUALITIES.map((value) => ({
  value,
  label: QUALITY_LABELS[value],
}));

/** The frame rates offered; the saved one is added when it is not among them. */
export const FPS_PRESETS: readonly number[] = [30, 60, 90, 120, 144, 165, 240];

/** The frame rate choices for a saved value: the presets plus the saved one, ascending. */
export function fpsOptions(saved: number): number[] {
  const valid = Number.isInteger(saved) && saved >= FPS_MIN && saved <= FPS_MAX;
  const values = valid && !FPS_PRESETS.includes(saved) ? [...FPS_PRESETS, saved] : [...FPS_PRESETS];
  return values.sort((a, b) => a - b);
}

/**
 * The screen choices for a saved value: the listed targets, portal first, plus the saved monitor
 * when it is not connected now, so the control never shows a value that is not the saved one.
 */
export function captureOptions(targets: readonly CaptureTarget[], saved: string): CaptureTarget[] {
  const portal = targets.filter((t) => t.id === PORTAL);
  const others = targets.filter((t) => t.id !== PORTAL);
  const options = [...(portal.length > 0 ? portal : [{ id: PORTAL, label: "Choose with the desktop (portal)" }]), ...others];
  if (!options.some((t) => t.id === saved)) options.push({ id: saved, label: `${saved} (not connected)` });
  return options;
}

/** What a screen choice means, in one sentence. */
export function captureExplanation(capture: string): string {
  return capture === PORTAL
    ? "The desktop asks which screen or window to share the first time the replay starts, and remembers the answer."
    : `Records the ${capture} monitor directly, without asking the desktop. GPU Screen Recorder may ask for your password to do this.`;
}

/** What happens after the screen changed from `before` to `after`; `null` when nothing does. */
export function captureChangeNotice(before: string, after: string): string | null {
  if (before === after) return null;
  return after === PORTAL
    ? "The desktop will ask again which screen to share the next time the replay starts."
    : "The saved desktop choice was cleared: choosing the portal again makes the desktop ask which screen to share.";
}
