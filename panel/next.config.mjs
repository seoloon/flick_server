/** @type {import('next').NextConfig} */
const nextConfig = {
  // The Docker image sets NEXT_OUTPUT=standalone; a plain checkout builds the regular output.
  output: process.env.NEXT_OUTPUT === "standalone" ? "standalone" : undefined,
  poweredByHeader: false,
  reactStrictMode: true,
  async headers() {
    return [
      {
        // Pages and API responses are never cached; hashed static assets and fonts keep their own policy.
        source: "/((?!_next/static|fonts/).*)",
        headers: [
          { key: "Cache-Control", value: "no-store" },
          { key: "X-Frame-Options", value: "DENY" },
          { key: "X-Content-Type-Options", value: "nosniff" },
          { key: "Referrer-Policy", value: "no-referrer" },
          {
            key: "Content-Security-Policy",
            // Next injects inline bootstrap scripts; everything else is same-origin.
            value:
              "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'",
          },
        ],
      },
    ];
  },
};

export default nextConfig;
