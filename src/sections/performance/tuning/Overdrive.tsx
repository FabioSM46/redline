import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { OVERDRIVE_ANCHOR } from "./block.ts";
import { type OverdrivePlan, type OverdriveResult, overdriveView, planExpired, type TuningStatus } from "../../../tuning.ts";

type Step =
  | { status: "idle" }
  | { status: "preparing" }
  | { status: "plan"; plan: OverdrivePlan }
  | { status: "enabling" }
  | { status: "done" }
  | { status: "error"; message: string };

/**
 * Overdrive (the `amdgpu` kernel bit LACT sets for the next boot). Enabling it is two steps: the
 * plan of what LACT will do (`prepare_enable_overdrive`, changes nothing), then, only after the
 * user has read it, `enable_overdrive` with that plan's token. System-wide, not per GPU.
 */
export function OverdriveBlock({
  status,
  locked,
  onBusy,
  onChanged,
}: {
  status: TuningStatus;
  locked: boolean;
  /** Enabling can take minutes (LACT rebuilds the boot files): the page locks meanwhile. */
  onBusy: (busy: boolean) => void;
  /** Overdrive changed: the page reads the status again. */
  onChanged: () => void;
}) {
  const [step, setStep] = useState<Step>({ status: "idle" });
  const view = overdriveView(status);

  const prepare = () => {
    setStep({ status: "preparing" });
    invoke<OverdrivePlan>("prepare_enable_overdrive").then(
      (plan) => setStep({ status: "plan", plan }),
      (e) => setStep({ status: "error", message: String(e) }),
    );
  };

  const enable = (plan: OverdrivePlan) => {
    if (planExpired(plan, Date.now())) {
      setStep({ status: "error", message: "The plan expired before it was confirmed: review it again." });
      return;
    }
    setStep({ status: "enabling" });
    onBusy(true);
    invoke<OverdriveResult>("enable_overdrive", { token: plan.token })
      .then(
        () => {
          setStep({ status: "done" });
          onChanged();
        },
        (e) => setStep({ status: "error", message: String(e) }),
      )
      .finally(() => onBusy(false));
  };

  return (
    <div className="tuning-block" id={OVERDRIVE_ANCHOR} tabIndex={-1}>
      <h2 className="record-title">Overdrive</h2>
      <div className="setting setting-wide">
        <div className="setting-text">
          <span className="setting-title">{overdriveTitle(view, step)}</span>
          {view === "disabled" && step.status !== "done" && (
            <span className="setting-description">
              Overdrive lets LACT change clocks and voltages and, on recent GPUs, set a fan curve: their firmware
              accepts fan curves only with overdrive on. It is a kernel parameter that LACT sets for the next boot,
              so a reboot is needed before it takes effect.
            </span>
          )}
          {view === "unknown" && (
            <span className="setting-description">LACT does not report whether overdrive is enabled.</span>
          )}
          {step.status === "error" && (
            <span className="setting-error" role="alert">
              {step.message}
            </span>
          )}
        </div>
        {view === "disabled" && step.status !== "done" && (
          <div className="setting-control">
            {step.status === "plan" ? (
              <OverdrivePlanView plan={step.plan} locked={locked} onConfirm={enable} onCancel={() => setStep({ status: "idle" })} />
            ) : step.status === "enabling" ? (
              <p className="setting-note" role="status" aria-busy="true">
                LACT is updating the boot files. This can take a few minutes: keep Redline open.
              </p>
            ) : (
              <button
                type="button"
                className="button"
                disabled={locked}
                aria-disabled={step.status === "preparing" || undefined}
                onClick={() => step.status !== "preparing" && prepare()}
              >
                {step.status === "preparing" ? "Reading what will change…" : "Enable overdrive…"}
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function overdriveTitle(view: ReturnType<typeof overdriveView>, step: Step): string {
  if (view === "reboot" || step.status === "done") return "Reboot to apply the overdrive change.";
  switch (view) {
    case "enabled":
      return "Overdrive is enabled.";
    case "disabled":
      return "Overdrive is off.";
    case "unknown":
      return "Overdrive state unknown.";
  }
}

/** The in-page confirmation: what LACT will do, in order, and the reboot it needs. */
function OverdrivePlanView({
  plan,
  locked,
  onConfirm,
  onCancel,
}: {
  plan: OverdrivePlan;
  locked: boolean;
  onConfirm: (plan: OverdrivePlan) => void;
  onCancel: () => void;
}) {
  return (
    <div className="tuning-plan" role="group" aria-label="Enable overdrive">
      <p className="setting-note">LACT will:</p>
      <ol className="shortcut-steps">
        {plan.steps.map((text) => (
          <li key={text}>{text}</li>
        ))}
      </ol>
      {plan.reboot_required && <p className="setting-note">Nothing takes effect until you reboot.</p>}
      <div className="tuning-actions">
        <button type="button" className="button button-primary" disabled={locked} onClick={() => onConfirm(plan)}>
          Enable overdrive
        </button>
        <button type="button" className="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
