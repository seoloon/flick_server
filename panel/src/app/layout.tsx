import type { Metadata, Viewport } from "next";

import "@/styles/tokens.css";
import "@/styles/flick-components.css";
import "@/styles/panel.css";

export const metadata: Metadata = {
  title: { default: "Flick Panel", template: "%s · Flick Panel" },
  description: "Web panel for Flick Server",
  icons: { icon: "/flick-mark.svg" },
  robots: { index: false, follow: false },
};

export const viewport: Viewport = {
  colorScheme: "dark",
  themeColor: "#0a0a0a",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body className="fk">{children}</body>
    </html>
  );
}
