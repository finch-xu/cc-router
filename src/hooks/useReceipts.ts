import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { ReceiptRange } from "@/types";

const RECEIPTS_KEY = "receipts";

export function useReceipt(range: ReceiptRange) {
  return useQuery({
    queryKey: [RECEIPTS_KEY, "summary", range],
    queryFn: () => api.getReceiptSummary(range),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}
