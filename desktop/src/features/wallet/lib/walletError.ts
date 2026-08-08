import type { WalletCommandError } from "../types";

export function walletCommandError(error: unknown): WalletCommandError {
  if (error instanceof Error) {
    const payload = "payload" in error ? error.payload : null;
    if (payload && typeof payload === "object") {
      return {
        ...(payload as WalletCommandError),
        message:
          typeof (payload as WalletCommandError).message === "string"
            ? (payload as WalletCommandError).message
            : error.message,
      };
    }
    return { message: error.message };
  }
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
