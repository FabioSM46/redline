import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { OVERDRIVE_ANCHOR, type TuningBlockProps } from "./block.ts";
import {
  dependencyAction,
  dependencyDetail,
  notEnabledByLact,
  type OverdriveAction,
  overdriveAction,
  overdriveBlocked,
  type OverdriveDependency,
  type OverdrivePlan,
  type OverdriveResult,
  overdriveView,
  type PendingChange,
  type PendingState,
  planConfirmable,
  planExpired,
  type TuningStatus,
} from "../../../tuning.ts";

type Step =
  | { status: "idle" }
  | { status: "preparing" }
  | { status: "plan"; plan: OverdrivePlan }
  | { status: "changing" }
  | { status: "done" }
  | { status: "error"; message: string };

/**
 * Overdrive (the `amdgpu` kernel bit LACT sets for the next boot). One switch for both directions,
 * each in two steps: the plan of what LACT will do (`prepare_*_overdrive`, changes nothing), then,
 * only after the user has read it, `enable_overdrive` / `disable_overdrive` with that plan's token.
 * System-wide, not per GPU. A disable plan lists what depends on overdrive on every GPU and cannot
 * be confirmed until the list is empty: each item goes back to default through the usual pending
 * change, and the plan is read again once that change settles.
 */
export function OverdriveBlock({
  status,
  locked,
  apply,
  settled,
  onBusy,
  onChanged,
}: {
  status: TuningStatus;
  locked: boolean;
  apply: TuningBlockProps["apply"];
  /** The last change that settled: a confirmed or reverted reset may change the list. */
  settled: PendingState["settled"];
  /** The change can take minutes (LACT rebuilds the boot files): the page locks meanwhile. */
  onBusy: (busy: boolean) => void;
  /** Overdrive changed: the page reads the status again. */
  onChanged: () => void;
}) {
  const [step, setStep] = useState<Step>({ status: "idle" });
  // A refusal that holds until LACT's state changes, which only a reboot does.
  const [refusal, setRefusal] = useState<string | null>(null);
  const view = overdriveView(status);
  const action = overdriveAction(view);
  const running = step.status === "preparing" || step.status === "changing";
  const planOpen = step.status === "plan";
  const blocked = planOpen ? null : overdriveBlocked(view, { running, locked, refusal });

  const fail = (e: unknown) => {
    const message = String(e);
    if (notEnabledByLact(message)) {
      setRefusal(message);
      setStep({ status: "idle" });
    } else {
      setStep({ status: "error", message });
    }
  };

  const prepare = (direction: OverdriveAction) => {
    setStep({ status: "preparing" });
    invoke<OverdrivePlan>(`prepare_${direction}_overdrive`).then((plan) => setStep({ status: "plan", plan }), fail);
  };

  const change = (plan: OverdrivePlan) => {
    if (planExpired(plan, Date.now())) {
      setStep({ status: "error", message: "The plan expired before it was confirmed: review it again." });
      return;
    }
    setStep({ status: "changing" });
    onBusy(true);
    invoke<OverdriveResult>(`${plan.action}_overdrive`, { token: plan.token })
      .then(
        () => {
          setStep({ status: "done" });
          onChanged();
        },
        fail,
      )
      .finally(() => onBusy(false));
  };

  // Each settled reset, kept or reverted, reads the list again. The open plan stays on screen until
  // the new one arrives, so the item and its focus do not jump. Only `settled` starts this.
  const disablePlanOpen = step.status === "plan" && step.plan.action === "disable";
  const planOpenRef = useRef(disablePlanOpen);
  planOpenRef.current = disablePlanOpen;
  useEffect(() => {
    if (settled === null || !planOpenRef.current) return;
    let current = true;
    invoke<OverdrivePlan>("prepare_disable_overdrive").then(
      (plan) => current && planOpenRef.current && setStep({ status: "plan", plan }),
      (e) => current && fail(e),
    );
    return () => {
      current = false;
    };
  }, [settled]);

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
          {view === "enabled" && step.status !== "done" && (
            <span className="setting-description">
              Turning it off stops clock and voltage changes and, on recent GPUs, fan curves. LACT updates the boot
              files, so a reboot is needed before it takes effect.
            </span>
          )}
          {step.status === "changing" && (
            <span className="setting-description" role="status" aria-busy="true">
              LACT is updating the boot files. This can take a few minutes: keep Redline open.
            </span>
          )}
          {blocked !== null && step.status !== "changing" && <span className="setting-description">{blocked}</span>}
          {step.status === "error" && (
            <span className="setting-error" role="alert">
              {step.message}
            </span>
          )}
        </div>
        <div className="setting-control">
          <input
            type="checkbox"
            role="switch"
            className="switch"
            aria-label="Overdrive"
            checked={status.overdrive_enabled === true}
            disabled={planOpen || blocked !== null || action === null || step.status === "done"}
            onChange={() => {
              if (!running && action !== null) prepare(action);
            }}
          />
        </div>
      </div>
      {step.status === "plan" && (
        <OverdrivePlanView
          plan={step.plan}
          locked={locked}
          apply={apply}
          onConfirm={change}
          onCancel={() => setStep({ status: "idle" })}
        />
      )}
    </div>
  );
}

function overdriveTitle(view: ReturnType<typeof overdriveView>, step: Step): string {
  if (view === "reboot" || step.status === "done") return "Reboot to apply the overdrive change.";
  switch (view) {
    case "enabled":
      return "Overdrive is on.";
    case "disabled":
      return "Overdrive is off.";
    case "unknown":
      return "Overdrive state unknown.";
  }
}

/** The in-page confirmation: what LACT will do, in order, what stops it, and the reboot it needs. */
function OverdrivePlanView({
  plan,
  locked,
  apply,
  onConfirm,
  onCancel,
}: {
  plan: OverdrivePlan;
  locked: boolean;
  apply: TuningBlockProps["apply"];
  onConfirm: (plan: OverdrivePlan) => void;
  onCancel: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const label = plan.action === "enable" ? "Enable overdrive" : "Turn overdrive off";

  const reset = (item: OverdriveDependency) => {
    setError(null);
    const command = item.kind === "clocks" ? "reset_clocks" : "reset_fan";
    apply(() => invoke<PendingChange>(command, { gpuId: item.gpu_id }), dependencyDetail(item)).then(setError);
  };

  return (
    <div className="tuning-plan" role="group" aria-label={label}>
      {plan.dependencies.length > 0 && (
        <>
          <p className="setting-note" role="status">
            Overdrive cannot be turned off yet: without it these settings would leave the fans or clocks uncontrolled.
            Put each one back to default; you keep or revert every result.
          </p>
          <ul className="tuning-dependencies">
            {plan.dependencies.map((item) => (
              <li className="tuning-dependency" key={`${item.gpu_id}:${item.kind}`}>
                <span>{item.sentence}</span>
                <button type="button" className="button" disabled={locked} onClick={() => reset(item)}>
                  {dependencyAction(item)}
                </button>
              </li>
            ))}
          </ul>
          {error !== null && (
            <p className="setting-error" role="alert">
              Not applied: {error}
            </p>
          )}
        </>
      )}
      <p className="setting-note">LACT will:</p>
      <ol className="shortcut-steps">
        {plan.steps.map((text) => (
          <li key={text}>{text}</li>
        ))}
      </ol>
      {plan.saved_profiles !== null && <p className="setting-note">{plan.saved_profiles}</p>}
      {plan.reboot_required && <p className="setting-note">Nothing takes effect until you reboot.</p>}
      <div className="tuning-actions">
        <button
          type="button"
          className="button button-primary"
          disabled={locked || !planConfirmable(plan)}
          onClick={() => onConfirm(plan)}
        >
          {label}
        </button>
        <button type="button" className="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
