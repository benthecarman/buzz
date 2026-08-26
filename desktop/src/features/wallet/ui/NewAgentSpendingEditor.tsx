import { LoaderCircle } from "lucide-react";
import { useEffect, useState } from "react";

import { getNwcWalletDefaultPolicy } from "../api";
import { walletErrorMessage } from "../lib/walletError";
import type { WalletNwcBudgetPeriod, WalletNwcDefaultPolicy } from "../types";
import {
  PolicyBudgetFields,
  PolicyModeControl,
  type WalletPolicyMode,
} from "./AgentPolicyControls";

export type NewAgentWalletPolicyDraft = {
  mode: WalletPolicyMode;
  amount: string;
  period: WalletNwcBudgetPeriod;
};

export function draftFromWalletPolicy(
  policy: WalletNwcDefaultPolicy,
): NewAgentWalletPolicyDraft {
  return {
    mode: policy.mode,
    amount: policy.budgetAmount === null ? "" : String(policy.budgetAmount),
    period: policy.budgetPeriod ?? "day",
  };
}

export function walletPolicyFromDraft(
  draft: NewAgentWalletPolicyDraft,
): WalletNwcDefaultPolicy | null {
  if (draft.mode === "manual") {
    return { mode: "manual", budgetAmount: null, budgetPeriod: null };
  }
  if (!/^\d+$/.test(draft.amount)) return null;
  const budgetAmount = Number(draft.amount);
  if (!Number.isSafeInteger(budgetAmount) || budgetAmount <= 0) return null;
  return {
    mode: "budget",
    budgetAmount,
    budgetPeriod: draft.period,
  };
}

export function useNewAgentWalletPolicy(onDirty?: () => void) {
  const [draft, setDraft] = useState<NewAgentWalletPolicyDraft | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getNwcWalletDefaultPolicy()
      .then((policy) => {
        if (!cancelled) setDraft(draftFromWalletPolicy(policy));
      })
      .catch((loadError) => {
        if (!cancelled) setError(walletErrorMessage(loadError));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const policy = draft ? walletPolicyFromDraft(draft) : null;
  return {
    editor: (
      <NewAgentSpendingEditor
        draft={draft}
        error={error}
        loading={loading}
        onChange={(nextDraft) => {
          onDirty?.();
          setDraft(nextDraft);
        }}
      />
    ),
    policy,
    valid: !loading && (error !== null || policy !== null),
  };
}

export function NewAgentSpendingEditor({
  draft,
  error,
  loading,
  onChange,
}: {
  draft: NewAgentWalletPolicyDraft | null;
  error: string | null;
  loading: boolean;
  onChange: (draft: NewAgentWalletPolicyDraft) => void;
}) {
  return (
    <div className="space-y-2" data-testid="create-agent-wallet-spending">
      <div>
        <p className="text-sm font-medium text-foreground">Wallet spending</p>
        <p className="mt-0.5 text-xs text-muted-foreground">
          This starts with the wallet default. Changes apply only to this agent.
        </p>
      </div>

      {loading ? (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <LoaderCircle className="h-4 w-4 animate-spin" />
          Loading wallet default…
        </div>
      ) : error || !draft ? (
        <p className="text-xs text-destructive">
          {error ?? "The wallet default is unavailable."} The saved default will
          still apply.
        </p>
      ) : (
        <div className="space-y-3">
          <PolicyModeControl
            legend="Approval mode for this agent"
            modeTestIdPrefix="create-agent-spending-mode"
            onValueChange={(mode) => onChange({ ...draft, mode })}
            value={draft.mode}
          />
          {draft.mode === "budget" ? (
            <div className="rounded-2xl border border-border bg-muted/30 px-4 py-3">
              <PolicyBudgetFields
                amount={draft.amount}
                amountLabel="Budget for this agent"
                budgetInputId="create-agent-spending-budget"
                onAmountChange={(amount) => onChange({ ...draft, amount })}
                onPeriodChange={(period) => onChange({ ...draft, period })}
                period={draft.period}
                periodLegend="Budget period for this agent"
                periodTestIdPrefix="create-agent-spending-period"
              />
              {walletPolicyFromDraft(draft) === null ? (
                <p className="mt-2 text-xs text-destructive">
                  Enter a whole-satoshi budget greater than zero.
                </p>
              ) : null}
            </div>
          ) : (
            <p className="text-xs text-muted-foreground">
              Every payment waits for your approval. Balance requests return
              zero.
            </p>
          )}
        </div>
      )}
    </div>
  );
}
