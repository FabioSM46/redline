// The game launch option's types and wording (D15). The backend composes the line and detects
// the tools: this file only mirrors `launch_options.rs` and `settings.rs` and says what each
// switch does. Pure.

/** `LaunchChoices` in src-tauri/src/settings.rs. */
export type LaunchChoices = {
  overlay: boolean;
  frame_generation: boolean;
  upscaler_upgrade: boolean;
  upscaler_indicator: boolean;
};

export type LaunchChoice = keyof LaunchChoices;

/** `Availability` in src-tauri/src/launch_options.rs. */
export type Availability = {
  available: boolean;
  /** What to install; `null` when `available`. */
  unavailable_reason: string | null;
};

/** `LaunchOptionsStatus` in src-tauri/src/launch_options.rs. */
export type LaunchOptionsStatus = {
  /** The saved choices, also the ones whose tool is missing. */
  choices: LaunchChoices;
  overlay: Availability;
  frame_generation: Availability;
  /** Which Proton builds upgrade the game's upscaler. */
  upscaler_upgrade_note: string;
  /** Which Proton builds show the upscaler watermark. */
  upscaler_indicator_note: string;
  /** The line to paste into the launcher. */
  command: string;
};

/** The switches, in the order they are shown. The words are Redline's own: no product is named. */
export const LAUNCH_SWITCHES: readonly { choice: LaunchChoice; label: string; description: string }[] = [
  { choice: "overlay", label: "Overlay", description: "Starts the game with the in-game overlay." },
  {
    choice: "frame_generation",
    label: "Frame generation",
    description: "Adds generated frames between the ones the game draws.",
  },
  {
    choice: "upscaler_upgrade",
    label: "Upscaler upgrade",
    description:
      "Replaces the game's upscaler with a newer one. Needs a game whose upscaler is version 3.1 or later, and changes how the game renders.",
  },
  {
    choice: "upscaler_indicator",
    label: "Upscaler watermark",
    description: "Shows a watermark in the game, only while the upscaler upgrade is active.",
  },
];

/** A switch as the block shows it. */
export type LaunchSwitch = {
  choice: LaunchChoice;
  label: string;
  description: string;
  /** On only when the choice is saved and its tool is there: a missing tool composes nothing. */
  checked: boolean;
  /** The tool is missing: a disabled feature, not an error. */
  disabled: boolean;
  /** The backend's sentence under the description: what to install, or which games show it. */
  note: string | null;
};

/** Whether the tool behind `choice` is installed. The upgrade and the watermark have none: a Proton build reads them or not. */
function availability(status: LaunchOptionsStatus, choice: LaunchChoice): Availability {
  if (choice === "overlay") return status.overlay;
  if (choice === "frame_generation") return status.frame_generation;
  return { available: true, unavailable_reason: null };
}

/** The backend's sentence about the Proton builds, for the two choices that have no tool. */
function buildNote(status: LaunchOptionsStatus, choice: LaunchChoice): string | null {
  if (choice === "upscaler_upgrade") return status.upscaler_upgrade_note;
  if (choice === "upscaler_indicator") return status.upscaler_indicator_note;
  return null;
}

/** The four switches for `status`. */
export function launchSwitches(status: LaunchOptionsStatus): LaunchSwitch[] {
  return LAUNCH_SWITCHES.map((row) => {
    const { available, unavailable_reason } = availability(status, row.choice);
    const note = buildNote(status, row.choice) ?? unavailable_reason;
    return {
      ...row,
      checked: available && status.choices[row.choice],
      disabled: !available,
      note: note || null,
    };
  });
}

/**
 * What `set_launch_options` receives when one switch changes: all four saved choices, with this
 * one replaced. The others are sent as saved, also those shown off because their tool is missing.
 */
export function withChoice(choices: LaunchChoices, choice: LaunchChoice, on: boolean): LaunchChoices {
  return { ...choices, [choice]: on };
}
