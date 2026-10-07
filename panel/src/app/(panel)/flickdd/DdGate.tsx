"use client";

import type { ReactNode } from "react";

import { Notice } from "@/components/flick/ui";
import type { DdOverview } from "@/lib/types";
import { ADMIN_ERROR_TEXT, useAdmin } from "@/lib/use-admin";

/** Shows one notice instead of the panels when FlickDD is switched off on the server. */
export function DdGate({ children }: { children: ReactNode }) {
  const { error } = useAdmin<DdOverview>("overview", 10_000, "flickdd");
  if (error === "DD_DISABLED") return <Notice tone="warn">{ADMIN_ERROR_TEXT.DD_DISABLED}</Notice>;
  return <>{children}</>;
}
