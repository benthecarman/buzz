const bitcoinAmountFormatter = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 0,
});
const usdAmountFormatter = new Intl.NumberFormat(undefined, {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

let bitcoinUsdRate: number | null = null;

export function setBitcoinUsdRate(rate: number | null): void {
  bitcoinUsdRate =
    rate !== null && Number.isFinite(rate) && rate > 0 ? rate : null;
}

export function formatSatsAsUsd(
  amount: number,
  rate = bitcoinUsdRate,
): string | null {
  if (rate === null || !Number.isFinite(amount)) return null;
  return usdAmountFormatter.format((amount / 100_000_000) * rate);
}

export function formatBitcoin(amount: number | null | undefined): string {
  if (amount === null || amount === undefined) return "₿ —";
  return `₿ ${bitcoinAmountFormatter.format(amount)}`;
}
