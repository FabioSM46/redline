import type { PendingChange, TuningStatus } from "../../../tuning.ts";

/**
 * What the Tuning page gives each of its blocks (power limit today; the fan editor of #60 and the
 * profiles of #61 plug in the same way). A block shows its part of `status` and changes it only
 * through `apply`, which opens the confirmation panel.
 */
export type TuningBlockProps = {
  gpuId: string;
  status: TuningStatus;
  /** A change is waiting for confirmation (on any GPU: lactd holds one) or being sent: no input. */
  locked: boolean;
  /**
   * Sends one setter that answers with a `PendingChange`. `detail` is what the confirmation
   * panel says changed ("250 W → 200 W"). Resolves to null once the change is pending, or to the
   * sentence that says why it was not applied.
   */
  apply: (send: () => Promise<PendingChange>, detail: string) => Promise<string | null>;
};
