import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  Copy,
  Eye,
  EyeOff,
  LoaderCircle,
  RefreshCw,
  Send,
  TriangleAlert,
} from "lucide-react";
import { toast } from "sonner";

import { writeTextToClipboard } from "@/shared/lib/clipboard";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { StyledQrCode } from "@/shared/ui/styled-qr-code";
import { SettingsSectionHeader } from "@/features/settings/ui/SettingsSectionHeader";
import {
  analyzeWalletDestination,
  createWalletReceiveRequest,
  getPendingWalletSend,
  getWalletStatus,
  listWalletTransactions,
  pollWalletUpdates,
  revealWalletRecoveryPhrase,
  sendWalletPayment,
} from "../api";
import { formatBitcoin } from "../lib/formatBitcoin";
import { parseWholeBitcoinAmount } from "../lib/profileZap";
import { walletCommandError, walletErrorMessage } from "../lib/walletError";
import type {
  WalletDestinationAnalysis,
  WalletFundingRequest,
  WalletStatus,
  WalletTransaction,
} from "../types";

const WALLET_POLL_INTERVAL_MS = 5_000;
const WALLET_POLL_MAX_BACKOFF_MS = 60_000;
type WalletAction = "fund" | "transfer";

function WalletLoading() {
  return (
    <div
      className="flex min-h-40 items-center justify-center gap-2 text-sm text-muted-foreground"
      data-testid="wallet-loading"
    >
      <LoaderCircle className="h-4 w-4 animate-spin" />
      Loading wallet…
    </div>
  );
}

function BalanceCard({
  activeAction,
  funding,
  onFund,
  onRefresh,
  onTransfer,
  refreshing,
  status,
}: {
  activeAction: WalletAction | null;
  funding: boolean;
  onFund: () => void;
  onRefresh: () => void;
  onTransfer: () => void;
  refreshing: boolean;
  status: WalletStatus;
}) {
  const reservedFunds = Math.max(0, status.balance - status.spendableBalance);

  return (
    <div className="rounded-2xl border border-border/70 bg-background p-5">
      <div className="flex items-start justify-between gap-3">
        <div>
          <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
            Spendable balance
          </p>
          <p
            className="mt-1 text-3xl font-semibold tracking-tight"
            data-testid="wallet-spendable-balance"
          >
            {formatBitcoin(status.spendableBalance)}
          </p>
          {reservedFunds > 0 ? (
            <p
              className="mt-1 text-xs text-muted-foreground"
              data-testid="wallet-reserved-funds"
            >
              Reserved funds: {formatBitcoin(reservedFunds)}
            </p>
          ) : null}
        </div>
        <Button
          aria-label="Refresh wallet"
          disabled={refreshing}
          onClick={onRefresh}
          size="icon"
          type="button"
          variant="ghost"
        >
          <RefreshCw className={refreshing ? "animate-spin" : undefined} />
        </Button>
      </div>
      <div className="mt-5 grid gap-2 sm:grid-cols-2">
        <Button
          disabled={funding}
          onClick={onFund}
          type="button"
          variant={activeAction === "fund" ? "default" : "outline"}
        >
          {funding ? (
            <LoaderCircle className="animate-spin" />
          ) : (
            <ArrowDownToLine />
          )}
          Fund wallet
        </Button>
        <Button
          disabled={status.spendableBalance === 0}
          onClick={onTransfer}
          type="button"
          variant={activeAction === "transfer" ? "default" : "outline"}
        >
          <ArrowUpFromLine />
          Transfer out
        </Button>
      </div>
    </div>
  );
}

function FundWalletCard({
  funding,
  generating,
  onGenerate,
}: {
  funding: WalletFundingRequest | null;
  generating: boolean;
  onGenerate: () => Promise<void>;
}) {
  async function copyFundingRequest() {
    if (!funding) return;
    await writeTextToClipboard(funding.bip321Uri);
    toast.success("Funding instruction copied");
  }

  return (
    <div className="rounded-2xl border border-border/70 bg-background p-5">
      <div>
        <h3 className="text-sm font-semibold">Fund wallet</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Fund from an existing Lightning wallet.
        </p>
      </div>
      {funding ? (
        <div className="mt-5 flex flex-col items-center gap-4">
          <div className="flex h-[312px] w-[312px] max-w-full items-center justify-center rounded-2xl border border-border/70 bg-white p-3">
            <StyledQrCode
              animate
              data-testid="wallet-receive-qr"
              size={288}
              title="Wallet funding QR code"
              value={funding.bip321Uri}
            />
          </div>
          <Button
            onClick={() => void copyFundingRequest()}
            type="button"
            variant="outline"
          >
            <Copy />
            Copy
          </Button>
          <p className="text-center text-xs text-muted-foreground">
            The amountless BIP-321 instruction includes BOLT12 and BOLT11.
            BOLT11 expires{" "}
            {new Date(funding.bolt11ExpiresAtMs).toLocaleString()}.
          </p>
        </div>
      ) : (
        <Button
          className="mt-4"
          disabled={generating}
          onClick={() => void onGenerate()}
          type="button"
          variant="outline"
        >
          {generating ? <LoaderCircle className="animate-spin" /> : null}
          Try again
        </Button>
      )}
    </div>
  );
}

function TransferOutCard({
  onPaymentComplete,
}: {
  onPaymentComplete: () => void;
}) {
  const [destination, setDestination] = useState("");
  const [amount, setAmount] = useState("");
  const [message, setMessage] = useState("");
  const [sending, setSending] = useState(false);
  const [analysis, setAnalysis] = useState<WalletDestinationAnalysis | null>(
    null,
  );
  const [requestId, setRequestId] = useState<string>(crypto.randomUUID());
  const [reconciling, setReconciling] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void getPendingWalletSend()
      .then(async (pending) => {
        if (!pending || cancelled) return;
        setDestination(pending.destination);
        setAmount(pending.amount === null ? "" : String(pending.amount));
        setMessage(pending.message ?? "");
        setRequestId(pending.requestId);
        setReconciling(true);
        const restoredAnalysis = await analyzeWalletDestination(
          pending.destination,
        );
        if (!cancelled) setAnalysis(restoredAnalysis);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  function resetAnalysis() {
    if (!reconciling) setAnalysis(null);
  }

  const parsedAmount = amount.trim()
    ? parseWholeBitcoinAmount(amount.trim())
    : null;

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (amount.trim() && parsedAmount === null) {
      toast.error("Enter a whole Bitcoin amount greater than zero.");
      return;
    }
    setSending(true);
    try {
      if (!analysis) {
        const nextAnalysis = await analyzeWalletDestination(destination.trim());
        const effectiveAmount = nextAnalysis.amount ?? parsedAmount;
        if (nextAnalysis.amount !== null && parsedAmount !== null) {
          toast.error("This destination already specifies its amount.");
          return;
        }
        if (effectiveAmount === null) {
          toast.error("Enter a whole Bitcoin amount greater than zero.");
          return;
        }
        if (
          (nextAnalysis.minAmount !== null &&
            effectiveAmount < nextAnalysis.minAmount) ||
          (nextAnalysis.maxAmount !== null &&
            effectiveAmount > nextAnalysis.maxAmount)
        ) {
          toast.error(
            "The amount is outside this destination's allowed range.",
          );
          return;
        }
        if (
          nextAnalysis.expiresAtMs !== null &&
          nextAnalysis.expiresAtMs <= Date.now()
        ) {
          toast.error("This payment destination has expired.");
          return;
        }
        setAnalysis(nextAnalysis);
        return;
      }
      const payment = await sendWalletPayment({
        destination: analysis.normalizedDestination,
        amount: parsedAmount,
        message: message.trim() || null,
        requestId,
      });
      if (payment.status === "completed") {
        toast.success(`Payment completed: ${formatBitcoin(payment.amount)}`);
        setDestination("");
        setAmount("");
        setMessage("");
        setAnalysis(null);
        setReconciling(false);
        setRequestId(crypto.randomUUID());
        onPaymentComplete();
      } else if (payment.status === "failed") {
        toast.error(payment.statusMessage || "The payment failed.");
        setReconciling(false);
        setRequestId(crypto.randomUUID());
      } else {
        toast.warning("The payment is pending. Buzz will reconcile it.");
        setReconciling(true);
      }
    } catch (error) {
      const commandError = walletCommandError(error);
      if (commandError.code === "payment_status_unknown") {
        setReconciling(true);
      } else if (commandError.code === "payment_failed") {
        setReconciling(false);
        setRequestId(crypto.randomUUID());
      }
      toast.error(walletErrorMessage(error));
    } finally {
      setSending(false);
    }
  }

  return (
    <form
      className="rounded-2xl border border-border/70 bg-background p-5"
      onSubmit={(event) => void submit(event)}
    >
      <div className="flex items-start justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold">Transfer out</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Send to a Lightning Address, BOLT11 invoice, BOLT12 offer, or
            BIP-321 URI.
          </p>
        </div>
        <ArrowUpFromLine className="h-5 w-5 text-muted-foreground" />
      </div>
      <div className="mt-4 space-y-2">
        <Input
          aria-label="Lightning destination"
          disabled={reconciling}
          onChange={(event) => {
            setDestination(event.target.value);
            resetAnalysis();
          }}
          placeholder="name@example.com, lnbc…, lno…, bitcoin:…"
          required
          value={destination}
        />
        <div className="grid gap-2 sm:grid-cols-2">
          <Input
            aria-label="Bitcoin amount"
            disabled={reconciling}
            inputMode="numeric"
            min={1}
            onChange={(event) => {
              setAmount(event.target.value);
              resetAnalysis();
            }}
            placeholder="Amount in ₿ (if needed)"
            type="text"
            value={amount}
          />
          <Input
            aria-label="Transfer note"
            disabled={reconciling}
            maxLength={200}
            onChange={(event) => {
              setMessage(event.target.value);
              resetAnalysis();
            }}
            placeholder="Optional note"
            value={message}
          />
        </div>
        {analysis ? (
          <div
            className="rounded-xl border border-border/70 bg-muted/30 p-3 text-sm"
            data-testid="wallet-send-confirmation"
          >
            <p className="font-medium">Confirm payment</p>
            {analysis.description ? (
              <p className="mt-1 text-muted-foreground">
                {analysis.description}
              </p>
            ) : null}
            <p className="mt-1 break-all text-xs text-muted-foreground">
              {analysis.normalizedDestination}
            </p>
            <p className="mt-2">
              Amount:{" "}
              {formatBitcoin(analysis.amount ?? parsedAmount ?? undefined)}
            </p>
            <p className="mt-1 text-xs text-muted-foreground">
              Lexe calculates routing or on-chain fees during payment. The final
              fee appears in transaction history.
            </p>
          </div>
        ) : null}
        <Button
          className="w-full"
          disabled={sending || !destination.trim()}
          type="submit"
        >
          {sending ? <LoaderCircle className="animate-spin" /> : <Send />}
          {reconciling
            ? "Check payment"
            : analysis
              ? "Confirm and send"
              : "Review payment"}
        </Button>
      </div>
    </form>
  );
}

function RecoveryCard() {
  const [phrase, setPhrase] = useState<string | null>(null);
  const [revealing, setRevealing] = useState(false);
  const hideTimerRef = useRef<number | null>(null);
  const clipboardTimerRef = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (hideTimerRef.current !== null) {
        window.clearTimeout(hideTimerRef.current);
      }
      if (clipboardTimerRef.current !== null) {
        window.clearTimeout(clipboardTimerRef.current);
      }
    },
    [],
  );

  async function reveal() {
    if (phrase) {
      setPhrase(null);
      if (hideTimerRef.current !== null) {
        window.clearTimeout(hideTimerRef.current);
      }
      return;
    }
    setRevealing(true);
    try {
      const nextPhrase = await revealWalletRecoveryPhrase();
      setPhrase(nextPhrase);
      hideTimerRef.current = window.setTimeout(() => {
        setPhrase(null);
        hideTimerRef.current = null;
      }, 30_000);
    } catch (error) {
      toast.error(walletErrorMessage(error));
    } finally {
      setRevealing(false);
    }
  }

  return (
    <div className="rounded-2xl border border-border/70 bg-background p-5">
      <div>
        <h3 className="text-sm font-semibold">Recovery phrase</h3>
        <p className="mt-1 text-xs text-muted-foreground">
          Your Buzz identity secret can derive this wallet phrase. The phrase
          recovers the wallet only; it does not recover your Buzz identity.
        </p>
        {phrase ? (
          <div className="mt-3 rounded-xl border border-destructive/30 bg-destructive/5 p-3">
            <p className="select-all font-mono text-sm leading-6">{phrase}</p>
            <Button
              className="mt-2"
              onClick={() => {
                void writeTextToClipboard(phrase)
                  .then(() => {
                    toast.success(
                      "Recovery phrase copied; clipboard clears in 60 seconds",
                    );
                    if (clipboardTimerRef.current !== null) {
                      window.clearTimeout(clipboardTimerRef.current);
                    }
                    clipboardTimerRef.current = window.setTimeout(() => {
                      void writeTextToClipboard("");
                      clipboardTimerRef.current = null;
                    }, 60_000);
                  })
                  .catch(() => toast.error("Failed to copy recovery phrase"));
              }}
              size="sm"
              type="button"
              variant="outline"
            >
              <Copy />
              Copy phrase
            </Button>
          </div>
        ) : null}
        <Button
          className="mt-3"
          disabled={revealing}
          onClick={() => void reveal()}
          type="button"
          variant="outline"
        >
          {revealing ? (
            <LoaderCircle className="animate-spin" />
          ) : phrase ? (
            <EyeOff />
          ) : (
            <Eye />
          )}
          {phrase ? "Hide recovery phrase" : "Reveal recovery phrase"}
        </Button>
      </div>
    </div>
  );
}

function TransactionHistory({
  error,
  loading,
  onLoadMore,
  transactions,
  canLoadMore,
}: {
  error: string | null;
  loading: boolean;
  onLoadMore: () => void;
  transactions: WalletTransaction[];
  canLoadMore: boolean;
}) {
  return (
    <div className="rounded-2xl border border-border/70 bg-background p-5">
      <h3 className="text-sm font-semibold">Transaction history</h3>
      {error ? <p className="mt-2 text-xs text-destructive">{error}</p> : null}
      <div className="mt-3 divide-y divide-border/70">
        {transactions.length === 0 && !loading ? (
          <p className="py-6 text-center text-sm text-muted-foreground">
            No transactions yet.
          </p>
        ) : null}
        {transactions.map((transaction) => (
          <div
            className="flex items-center justify-between gap-3 py-3"
            key={transaction.id}
          >
            <div className="min-w-0">
              <p className="text-sm font-medium capitalize">
                {transaction.direction} · {transaction.status}
              </p>
              <p className="truncate text-xs text-muted-foreground">
                {transaction.note || transaction.statusMessage}
              </p>
              <p className="text-2xs text-muted-foreground">
                {new Date(transaction.createdAtMs).toLocaleString()}
              </p>
            </div>
            <div className="shrink-0 text-right">
              <p className="text-sm font-medium">
                {transaction.amount === null
                  ? formatBitcoin(null)
                  : `${transaction.direction === "outbound" ? "−" : transaction.direction === "inbound" ? "+" : ""}${formatBitcoin(transaction.amount)}`}
              </p>
              {transaction.fees > 0 ? (
                <p className="text-2xs text-muted-foreground">
                  Fee {formatBitcoin(transaction.fees)}
                </p>
              ) : null}
            </div>
          </div>
        ))}
      </div>
      {canLoadMore ? (
        <Button
          className="mt-3 w-full"
          disabled={loading}
          onClick={onLoadMore}
          type="button"
          variant="outline"
        >
          {loading ? <LoaderCircle className="animate-spin" /> : null}
          Load more
        </Button>
      ) : null}
    </div>
  );
}

export function WalletSettingsPanel() {
  const [status, setStatus] = useState<WalletStatus | null>(null);
  const [funding, setFunding] = useState<WalletFundingRequest | null>(null);
  const [activeAction, setActiveAction] = useState<WalletAction | null>(null);
  const [transactions, setTransactions] = useState<WalletTransaction[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [generating, setGenerating] = useState(false);
  const [refreshingBalance, setRefreshingBalance] = useState(false);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const fundingStartBalanceRef = useRef<number | null>(null);

  const completeFundingIfReceived = useCallback(
    (nextBalance: number) => {
      const startingBalance = fundingStartBalanceRef.current;
      if (
        activeAction !== "fund" ||
        startingBalance === null ||
        nextBalance <= startingBalance
      ) {
        return;
      }
      fundingStartBalanceRef.current = null;
      setFunding(null);
      setActiveAction(null);
      toast.success(
        `Funds received: ${formatBitcoin(nextBalance - startingBalance)}`,
      );
    },
    [activeAction],
  );

  const loadHistory = useCallback(async (cursor?: string) => {
    setHistoryLoading(true);
    setHistoryError(null);
    try {
      const page = await listWalletTransactions(cursor);
      setTransactions((current) =>
        cursor ? [...current, ...page.transactions] : page.transactions,
      );
      setNextCursor(page.nextCursor);
    } catch (loadError) {
      const message = walletErrorMessage(loadError);
      setHistoryError(message);
      if (cursor) toast.error(message);
    } finally {
      setHistoryLoading(false);
    }
  }, []);

  const initialize = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setStatus(await getWalletStatus());
    } catch (loadError) {
      setError(walletErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
    void loadHistory();
  }, [loadHistory]);

  useEffect(() => {
    void initialize();
  }, [initialize]);

  const walletReady = !loading && status !== null;

  useEffect(() => {
    if (!walletReady) return;

    let cancelled = false;
    let pollRunning = false;
    let timeoutId: number | null = null;
    let consecutiveFailures = 0;

    function clearScheduledPoll() {
      if (timeoutId !== null) {
        window.clearTimeout(timeoutId);
        timeoutId = null;
      }
    }

    function schedulePoll(delayMs: number) {
      if (cancelled || document.visibilityState !== "visible") return;
      clearScheduledPoll();
      timeoutId = window.setTimeout(() => {
        timeoutId = null;
        void poll();
      }, delayMs);
    }

    async function poll() {
      if (cancelled || pollRunning || document.visibilityState !== "visible") {
        return;
      }
      pollRunning = true;
      try {
        const changed = await pollWalletUpdates();
        consecutiveFailures = 0;
        if (changed) {
          const [nextStatus, page] = await Promise.all([
            getWalletStatus(),
            listWalletTransactions(undefined, false),
          ]);
          if (!cancelled) {
            setStatus(nextStatus);
            setTransactions((current) => {
              const newestIds = new Set(
                page.transactions.map((transaction) => transaction.id),
              );
              return [
                ...page.transactions,
                ...current.filter(
                  (transaction) => !newestIds.has(transaction.id),
                ),
              ];
            });
            completeFundingIfReceived(nextStatus.balance);
          }
        }
      } catch {
        consecutiveFailures += 1;
      } finally {
        pollRunning = false;
        const backoffMultiplier = 2 ** Math.min(consecutiveFailures, 4);
        schedulePoll(
          Math.min(
            WALLET_POLL_INTERVAL_MS * backoffMultiplier,
            WALLET_POLL_MAX_BACKOFF_MS,
          ),
        );
      }
    }

    function handleVisibilityChange() {
      if (document.visibilityState === "visible") {
        schedulePoll(0);
      } else {
        clearScheduledPoll();
      }
    }

    document.addEventListener("visibilitychange", handleVisibilityChange);
    schedulePoll(WALLET_POLL_INTERVAL_MS);

    return () => {
      cancelled = true;
      clearScheduledPoll();
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [completeFundingIfReceived, walletReady]);

  async function refreshStatusAndHistory() {
    setRefreshingBalance(true);
    try {
      const [nextStatus] = await Promise.all([
        getWalletStatus(),
        loadHistory(),
      ]);
      setStatus(nextStatus);
      completeFundingIfReceived(nextStatus.balance);
    } catch (refreshError) {
      toast.error(walletErrorMessage(refreshError));
    } finally {
      setRefreshingBalance(false);
    }
  }

  async function createReceiveRequest() {
    setGenerating(true);
    try {
      setFunding(await createWalletReceiveRequest());
    } catch (receiveError) {
      toast.error(walletErrorMessage(receiveError));
    } finally {
      setGenerating(false);
    }
  }

  function showFunding() {
    fundingStartBalanceRef.current = status?.balance ?? null;
    setActiveAction("fund");
    if (!funding && !generating) {
      void createReceiveRequest();
    }
  }

  if (loading) return <WalletLoading />;

  if (error || !status) {
    return (
      <div
        className="rounded-2xl border border-destructive/35 bg-destructive/5 p-5"
        data-testid="wallet-error"
      >
        <div className="flex items-start gap-3">
          <TriangleAlert className="mt-0.5 h-5 w-5 text-destructive" />
          <div>
            <p className="text-sm font-medium">Wallet unavailable</p>
            <p className="mt-1 text-xs text-muted-foreground">{error}</p>
            <Button
              className="mt-3"
              onClick={() => void initialize()}
              size="sm"
              type="button"
              variant="outline"
            >
              Try again
            </Button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <section className="space-y-4" data-testid="settings-wallet">
      <SettingsSectionHeader
        title="Wallet"
        description="Manage your self-custodial Bitcoin wallet."
      />
      <BalanceCard
        activeAction={activeAction}
        funding={generating}
        onFund={showFunding}
        onRefresh={() => void refreshStatusAndHistory()}
        onTransfer={() => setActiveAction("transfer")}
        refreshing={refreshingBalance}
        status={status}
      />
      {activeAction === "fund" ? (
        <FundWalletCard
          funding={funding}
          generating={generating}
          onGenerate={createReceiveRequest}
        />
      ) : null}
      {activeAction === "transfer" ? (
        <TransferOutCard
          onPaymentComplete={() => void refreshStatusAndHistory()}
        />
      ) : null}
      <RecoveryCard />
      <TransactionHistory
        canLoadMore={nextCursor !== null}
        error={historyError}
        loading={historyLoading}
        onLoadMore={() => {
          if (nextCursor) void loadHistory(nextCursor);
        }}
        transactions={transactions}
      />
    </section>
  );
}
