import { notFound } from "next/navigation";

import { PageHeader } from "@/components/flick/ui";
import { SCOPE_LEAD, isSettingsScope } from "@/lib/settings-form";

import { ScopeTabs } from "../ScopeTabs";
import { SettingsForm } from "../SettingsForm";

export const metadata = { title: "Settings" };

export default async function SettingsPage({ params }: { params: Promise<{ scope: string }> }) {
  const { scope } = await params;
  if (!isSettingsScope(scope)) notFound();
  return (
    <div className="panel-page">
      <PageHeader title="Settings" lead={SCOPE_LEAD[scope]} />
      <ScopeTabs current={scope} />
      {/* A new form per scope: edits never carry over from one scope to another. */}
      <SettingsForm key={scope} scope={scope} />
    </div>
  );
}
