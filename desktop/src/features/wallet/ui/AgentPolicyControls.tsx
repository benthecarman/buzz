import { ShieldCheck, Zap } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";

import { Input } from "@/shared/ui/input";
import { SegmentedControl } from "@/shared/ui/segmented-control";

import { getWalletStatus } from "../api";
import { formatBitcoin, formatSatsAsUsd } from "../lib/formatBitcoin";
import type { WalletNwcBudgetPeriod } from "../types";

export const MODE_OPTIONS = [
  { value: "manual", label: "Ask every time", Icon: ShieldCheck },
  { value: "budget", label: "Spending budget", Icon: Zap },
] as const;

export const PERIOD_OPTIONS = [
  { value: "hour", label: "Hour" },
  { value: "day", label: "Day" },
  { value: "week", label: "Week" },
  { value: "month", label: "Month" },
] as const;

export type WalletPolicyMode = "manual" | "budget";

/** Parse a budget text field, yielding 0 for anything but whole satoshis. */
export function parseBudgetAmount(amount: string): number {
  return /^\d+$/.test(amount) ? Number(amount) : 0;
}

/** Manual-vs-budget segmented control shared by every policy editor. */
export function PolicyModeControl({
  legend,
  modeTestIdPrefix,
  onValueChange,
  value,
}: {
  legend: string;
  modeTestIdPrefix: string;
  onValueChange: (value: WalletPolicyMode) => void;
  value: WalletPolicyMode;
}) {
  return (
    <SegmentedControl
      className="w-full lg:w-72"
      legend={legend}
      onValueChange={onValueChange}
      optionTestIdPrefix={modeTestIdPrefix}
      options={MODE_OPTIONS}
      size="wide"
      testId={modeTestIdPrefix}
      value={value}
    />
  );
}

/** Sats amount and reset-period fields shared by every policy editor. */
export function PolicyBudgetFields({
  amount,
  amountLabel,
  budgetInputId,
  children,
  onAmountChange,
  onPeriodChange,
  period,
  periodLegend,
  periodTestIdPrefix,
}: {
  amount: string;
  amountLabel: string;
  budgetInputId: string;
  children?: ReactNode;
  onAmountChange: (amount: string) => void;
  onPeriodChange: (period: WalletNwcBudgetPeriod) => void;
  period: WalletNwcBudgetPeriod;
  periodLegend: string;
  periodTestIdPrefix: string;
}) {
  const statusQuery = useQuery({
    queryKey: ["wallet-status", "agent-budget-context"],
    queryFn: getWalletStatus,
    retry: 1,
    staleTime: 15_000,
  });
  const spendableBalance = statusQuery.data?.spendableBalance ?? null;
  const parsedAmount = parseBudgetAmount(amount);
  const amountUsd = parsedAmount > 0 ? formatSatsAsUsd(parsedAmount) : null;

  return (
    <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)_auto] sm:items-end">
      <label
        className="space-y-1.5 text-xs font-medium"
        htmlFor={budgetInputId}
      >
        <span>
          Budget
          {spendableBalance !== null
            ? ` (of total: ${formatBitcoin(spendableBalance).replace("₿ ", "₿")})`
            : null}
        </span>
        <div className="relative">
          <span className="pointer-events-none absolute inset-y-0 left-3 flex items-center text-sm text-muted-foreground">
            ₿
          </span>
          <Input
            aria-label={amountLabel}
            className="pl-7"
            id={budgetInputId}
            inputMode="numeric"
            min={1}
            onChange={(event) => onAmountChange(event.target.value)}
            value={amount}
          />
        </div>
        {amountUsd ? (
          <span
            className="block text-2xs font-normal text-muted-foreground"
            data-testid={`${budgetInputId}-usd`}
          >
            {amountUsd}
          </span>
        ) : null}
      </label>
      <div className="space-y-1.5">
        <p className="text-xs font-medium">Reset every</p>
        <SegmentedControl
          className="w-full"
          legend={periodLegend}
          onValueChange={onPeriodChange}
          optionTestIdPrefix={periodTestIdPrefix}
          options={PERIOD_OPTIONS}
          size="wide"
          testId={periodTestIdPrefix}
          value={period}
        />
      </div>
      {children}
    </div>
  );
}
