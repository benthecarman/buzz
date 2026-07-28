import { useEffect, useState } from "react";

import { bitcoinCompileEnabled } from "./api";

export function useBitcoinCompileEnabled(): boolean {
  const [available, setAvailable] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void bitcoinCompileEnabled()
      .then((enabled) => {
        if (!cancelled) setAvailable(enabled);
      })
      .catch(() => {
        if (!cancelled) setAvailable(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return available;
}
