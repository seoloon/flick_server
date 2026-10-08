export function panelEnabled(env: Record<string, string | undefined>): boolean;
export function checkPanelEnv(env: Record<string, string | undefined>): {
  problems: string[];
  notes: string[];
};
