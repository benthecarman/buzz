import { useQuery } from "@tanstack/react-query";
import { useFeatureEnabled } from "@/shared/features/useFeatureEnabled";
import { setBitcoinUsdRate } from "./lib/formatBitcoin";

type CoinbaseSpotPriceResponse = {
  data?: {
    amount?: string;
    currency?: string;
  };
};

const BITCOIN_USD_SPOT_URL = "https://api.coinbase.com/v2/prices/BTC-USD/spot";

async function fetchBitcoinUsdRate(): Promise<number> {
  const response = await fetch(BITCOIN_USD_SPOT_URL, {
    headers: { Accept: "application/json" },
  });
  if (!response.ok) {
    throw new Error(`Bitcoin price request failed with ${response.status}.`);
  }
  const body = (await response.json()) as CoinbaseSpotPriceResponse;
  const rate = Number(body.data?.amount);
  if (body.data?.currency !== "USD" || !Number.isFinite(rate) || rate <= 0) {
    throw new Error("Bitcoin price response was invalid.");
  }
  return rate;
}

/** Keep the shared sats formatter current without blocking Bitcoin UI. */
export function useBitcoinUsdRate(): void {
  const bitcoinEnabled = useFeatureEnabled("bitcoin");
  const query = useQuery({
    enabled: bitcoinEnabled,
    queryKey: ["bitcoin-usd-rate"],
    queryFn: fetchBitcoinUsdRate,
    staleTime: 5 * 60_000,
    refetchInterval: 5 * 60_000,
    retry: 1,
  });
  setBitcoinUsdRate(bitcoinEnabled ? (query.data ?? null) : null);
}
