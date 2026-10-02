import { redirect } from "next/navigation";

import { hasSession } from "@/lib/guard";

import { LoginForm } from "./LoginForm";

export const metadata = { title: "Sign In" };
export const dynamic = "force-dynamic";

export default async function LoginPage() {
  if (await hasSession()) redirect("/");
  return (
    <div className="fk-ambient login">
      <div className="login__card fk-glass-strong">
        <LoginForm />
      </div>
    </div>
  );
}
