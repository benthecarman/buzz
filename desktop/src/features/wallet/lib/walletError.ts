import type { WalletCommandError } from "../types";

export function walletCommandError(error: unknown): WalletCommandError {
  if (error instanceof Error) return { message: error.message };
  if (typeof error === "string") return { message: error };
  if (error && typeof error === "object") return error as WalletCommandError;
  return {};
}

export function walletErrorMessage(error: unknown): string {
  return (
    walletCommandError(error).message ??
    "The wallet operation failed. Try again."
  );
}
