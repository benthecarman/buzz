import { useEffect, useState } from "react";

import { bitcoinCompileEnabled } from "./api";

let cachedBitcoinCompileEnabled: boolean | null = null;

export function useBitcoinCompileEnabled(): boolean {
  const [available, setAvailable] = useState(
    cachedBitcoinCompileEnabled ?? false,
  );

  useEffect(() => {
    if (cachedBitcoinCompileEnabled !== null) {
      setAvailable(cachedBitcoinCompileEnabled);
      return;
    }

    let cancelled = false;
    void bitcoinCompileEnabled()
      .then((enabled) => {
        cachedBitcoinCompileEnabled = enabled;
        if (!cancelled) setAvailable(enabled);
      })
      .catch(() => {
        cachedBitcoinCompileEnabled = false;
        if (!cancelled) setAvailable(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return available;
}
