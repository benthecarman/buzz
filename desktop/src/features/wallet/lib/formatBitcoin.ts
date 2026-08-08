const bitcoinAmountFormatter = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 0,
});

export function formatBitcoin(amount: number | null | undefined): string {
  if (amount === null || amount === undefined) return "₿ —";
  return `₿ ${bitcoinAmountFormatter.format(amount)}`;
}
