"use client";

import { useActionState, useState } from "react";

import { Wordmark } from "@/components/flick/icons";
import { Button, Notice, TextField } from "@/components/flick/ui";

import { type LoginState, login } from "./actions";

export function LoginForm() {
  const [state, action, pending] = useActionState<LoginState, FormData>(login, {});
  const [password, setPassword] = useState("");
  return (
    <>
      <div className="login__brand">
        <Wordmark height="1.5rem" />
        <h1 className="login__title">Flick Panel</h1>
        <p className="login__lead">Sign in to manage your Flick Server.</p>
      </div>
      <form action={action}>
        <TextField
          label="Password"
          name="password"
          type="password"
          value={password}
          onChange={setPassword}
          autoFocus
          autoComplete="current-password"
        />
        {state.error && <Notice tone="error">{state.error}</Notice>}
        <Button type="submit" variant="primary" size="lg" disabled={pending || !password}>
          {pending ? "Signing In" : "Sign In"}
        </Button>
      </form>
    </>
  );
}
