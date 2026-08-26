import { Check, LoaderCircle, Zap } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/shared/ui/button";

import { getNwcWalletDefaultPolicy, setNwcWalletDefaultPolicy } from "../api";
import { walletErrorMessage } from "../lib/walletError";
import type { WalletNwcBudgetPeriod, WalletNwcDefaultPolicy } from "../types";
import {
  parseBudgetAmount,
  PolicyBudgetFields,
  PolicyModeControl,
} from "./AgentPolicyControls";

function DefaultPolicyEditor({
  policy,
  onSaved,
}: {
  policy: WalletNwcDefaultPolicy;
  onSaved: (policy: WalletNwcDefaultPolicy) => void;
}) {
  const [mode, setMode] = useState(policy.mode);
  const [amount, setAmount] = useState(
    policy.budgetAmount === null ? "" : String(policy.budgetAmount),
  );
  const [period, setPeriod] = useState<WalletNwcBudgetPeriod>(
    policy.budgetPeriod ?? "day",
  );
  const [saving, setSaving] = useState(false);
  const parsedAmount = parseBudgetAmount(amount);
  const changed =
    mode !== policy.mode ||
    (mode === "budget" &&
      (parsedAmount !== policy.budgetAmount || period !== policy.budgetPeriod));

  async function save() {
    if (
      mode === "budget" &&
      (!Number.isSafeInteger(parsedAmount) || parsedAmount <= 0)
    ) {
      toast.error("Enter a whole-satoshi budget greater than zero.");
      return;
    }
    setSaving(true);
    try {
      const saved = await setNwcWalletDefaultPolicy({
        mode,
        budgetAmount: mode === "budget" ? parsedAmount : null,
        budgetPeriod: mode === "budget" ? period : null,
      });
      onSaved(saved);
      toast.success("Default agent budget was updated");
    } catch (error) {
      toast.error(walletErrorMessage(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="mt-5 rounded-xl border border-border/70 bg-muted/15 p-4"
      data-testid="wallet-default-agent-policy"
    >
      <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
        <div className="min-w-0">
          <p className="text-sm font-semibold">Default for new agents</p>
          <p className="mt-0.5 text-xs text-muted-foreground">
            This applies to agents you create or claim later. Existing agents
            keep their current budgets.
          </p>
        </div>
        <PolicyModeControl
          legend="Default approval mode for new agents"
          modeTestIdPrefix="wallet-default-agent-mode"
          onValueChange={setMode}
          value={mode}
        />
      </div>

      {mode === "budget" ? (
        <div className="mt-4 rounded-xl bg-background p-3 shadow-xs ring-1 ring-border/60">
          <PolicyBudgetFields
            amount={amount}
            amountLabel="Default budget for new agents"
            budgetInputId="wallet-default-agent-budget"
            onAmountChange={setAmount}
            onPeriodChange={setPeriod}
            period={period}
            periodLegend="Default budget period for new agents"
            periodTestIdPrefix="wallet-default-agent-period"
          >
            <Button
              disabled={!changed || saving}
              onClick={() => void save()}
              type="button"
            >
              {saving ? <LoaderCircle className="animate-spin" /> : <Check />}
              Save
            </Button>
          </PolicyBudgetFields>
        </div>
      ) : (
        <div className="mt-3 flex items-center justify-between gap-3">
          <p className="text-xs text-muted-foreground">
            New agents ask for approval before every payment.
          </p>
          {changed ? (
            <Button
              disabled={saving}
              onClick={() => void save()}
              size="sm"
              type="button"
              variant="outline"
            >
              {saving ? <LoaderCircle className="animate-spin" /> : <Check />}
              Save
            </Button>
          ) : null}
        </div>
      )}
    </div>
  );
}

export function AgentSpendingCard() {
  const [defaultPolicy, setDefaultPolicy] =
    useState<WalletNwcDefaultPolicy | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setDefaultPolicy(await getNwcWalletDefaultPolicy());
    } catch (loadError) {
      setError(walletErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div
      className="rounded-2xl border border-border/70 bg-background p-5"
      data-testid="wallet-agent-spending"
    >
      <div className="flex items-start gap-3">
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-amber-500/10 text-amber-600 dark:text-amber-400">
          <Zap className="h-4 w-4" />
        </div>
        <div>
          <h3 className="text-sm font-semibold">Default agent budget</h3>
          <p className="mt-1 max-w-2xl text-xs text-muted-foreground">
            Choose the approval policy for new agents. To change an existing
            agent's budget, open its edit screen and expand Advanced settings.
          </p>
        </div>
      </div>

      {loading ? (
        <div className="mt-5 flex items-center gap-2 text-xs text-muted-foreground">
          <LoaderCircle className="h-4 w-4 animate-spin" />
          Loading default…
        </div>
      ) : error ? (
        <div className="mt-4 flex items-center justify-between gap-3 rounded-xl bg-destructive/5 p-3 text-xs text-destructive">
          <span>{error}</span>
          <Button onClick={() => void load()} size="sm" variant="outline">
            Try again
          </Button>
        </div>
      ) : defaultPolicy ? (
        <DefaultPolicyEditor
          onSaved={setDefaultPolicy}
          policy={defaultPolicy}
        />
      ) : null}
    </div>
  );
}
