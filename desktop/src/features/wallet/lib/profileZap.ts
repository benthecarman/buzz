export function parseWholeBitcoinAmount(value: string): number | null {
  if (!/^[1-9]\d*$/.test(value)) return null;
  const amount = Number(value);
  return Number.isSafeInteger(amount) ? amount : null;
}
