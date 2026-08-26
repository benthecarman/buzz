import { Check, LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/shared/ui/button";
import { Progress } from "@/shared/ui/progress";

import { listNwcWalletClients, setNwcWalletPolicy } from "../api";
import { formatBitcoin } from "../lib/formatBitcoin";
import { walletErrorMessage } from "../lib/walletError";
import type {
  WalletNwcBudgetPeriod,
  WalletNwcClient,
  WalletNwcPolicyUpdate,
} from "../types";
import {
  parseBudgetAmount,
  PolicyBudgetFields,
  PolicyModeControl,
  type WalletPolicyMode,
} from "./AgentPolicyControls";

/**
 * Per-agent spending policy editor, embeddable outside wallet settings.
 *
 * Uses the same policy that NWC request handling enforces. Managed agents are
 * always NWC clients, so loading failures stay visible instead of hiding the
 * control.
 */
export function AgentSpendingEditor({
  agentName,
  agentPubkey,
}: {
  agentName: string;
  agentPubkey: string;
}) {
  const [client, setClient] = useState<WalletNwcClient | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [mode, setMode] = useState<WalletPolicyMode>("manual");
  const [amount, setAmount] = useState("");
  const [period, setPeriod] = useState<WalletNwcBudgetPeriod>("day");
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const clients = await listNwcWalletClients();
      const found = clients.find(
        (item) => item.agentPubkey === agentPubkey,
      ) ?? {
        agentPubkey,
        agentName,
        mode: "manual" as const,
        budgetAmount: null,
        budgetPeriod: null,
        spentAmount: 0,
        remainingAmount: null,
        periodEndsAtMs: null,
      };
      setClient(found);
      setMode(found.mode);
      setAmount(found.budgetAmount === null ? "" : String(found.budgetAmount));
      setPeriod(found.budgetPeriod ?? "day");
    } catch (error) {
      setClient(null);
      setLoadError(walletErrorMessage(error));
    } finally {
      setLoading(false);
    }
  }, [agentName, agentPubkey]);

  useEffect(() => {
    void load();
  }, [load]);

  if (loading) {
    return (
      <div
        className="flex items-center gap-2 text-xs text-muted-foreground"
        data-testid="edit-agent-wallet-spending"
      >
        <LoaderCircle className="h-4 w-4 animate-spin" />
        Loading wallet policy…
      </div>
    );
  }

  if (loadError || !client) {
    return (
      <div
        className="flex items-center justify-between gap-3 text-xs text-destructive"
        data-testid="edit-agent-wallet-spending"
      >
        <span>{loadError ?? "The wallet policy is unavailable."}</span>
        <Button onClick={() => void load()} size="sm" variant="outline">
          Try again
        </Button>
      </div>
    );
  }

  const parsedAmount = parseBudgetAmount(amount);
  const changed =
    mode !== client.mode ||
    (mode === "budget" &&
      (parsedAmount !== client.budgetAmount || period !== client.budgetPeriod));
  const usage =
    client.budgetAmount && client.budgetAmount > 0
      ? (client.spentAmount / client.budgetAmount) * 100
      : 0;

  async function save() {
    if (
      mode === "budget" &&
      (!Number.isSafeInteger(parsedAmount) || parsedAmount <= 0)
    ) {
      toast.error("Enter a whole-satoshi budget greater than zero.");
      return;
    }
    const update: WalletNwcPolicyUpdate = {
      agentPubkey,
      mode,
      budgetAmount: mode === "budget" ? parsedAmount : null,
      budgetPeriod: mode === "budget" ? period : null,
    };
    setSaving(true);
    try {
      const saved = await setNwcWalletPolicy(update);
      setClient(saved);
      toast.success(`${agentName}'s wallet policy was updated`);
    } catch (error) {
      toast.error(walletErrorMessage(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="space-y-1.5" data-testid="edit-agent-wallet-spending">
      <div className="flex flex-col gap-2 lg:flex-row lg:items-center lg:justify-between">
        <span className="text-sm font-medium text-foreground">
          Wallet spending
        </span>
        <PolicyModeControl
          legend={`Approval mode for ${agentName}`}
          modeTestIdPrefix={`edit-agent-spending-mode-${agentPubkey}`}
          onValueChange={setMode}
          value={mode}
        />
      </div>
      {mode === "budget" ? (
        <div className="space-y-3 rounded-2xl border border-border bg-muted/30 px-4 py-3">
          <PolicyBudgetFields
            amount={amount}
            amountLabel={`Budget for ${agentName}`}
            budgetInputId={`edit-agent-spending-budget-${agentPubkey}`}
            onAmountChange={setAmount}
            onPeriodChange={setPeriod}
            period={period}
            periodLegend={`Budget period for ${agentName}`}
            periodTestIdPrefix={`edit-agent-spending-period-${agentPubkey}`}
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
          {client.mode === "budget" && client.remainingAmount !== null ? (
            <div className="space-y-1.5">
              <div className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
                <span>{formatBitcoin(client.spentAmount)} used</span>
                <span>{formatBitcoin(client.remainingAmount)} left</span>
              </div>
              <Progress value={usage} />
              {client.periodEndsAtMs ? (
                <p className="text-right text-2xs text-muted-foreground">
                  Resets {new Date(client.periodEndsAtMs).toLocaleString()}
                </p>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : (
        <div className="flex items-center justify-between gap-3">
          <p className="text-xs text-muted-foreground">
            Every payment waits for your approval. Balance requests return zero.
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
      <p className="text-xs text-muted-foreground">
        Wallet settings controls the default for new agents.
      </p>
    </div>
  );
}
