"use client";
import { useActionState } from "react";
import { buttonClass } from "../../components/ui/button";
import { createProject } from "./actions";

/** Name, and an optional slug (made from the name when left empty). */
export function NewProjectForm() {
  const [state, action, pending] = useActionState<{ error?: string } | null, FormData>(createProject, null);
  return (
    <form action={action} className="mt-6 flex flex-wrap items-center gap-2" aria-label="New project">
      <label className="flex min-w-0 flex-[2_1_220px]">
        <span className="sr-only">Project name</span>
        <input name="name" placeholder="Project name" required maxLength={120} className="w-full" />
      </label>
      <label className="flex min-w-0 flex-[1_1_160px]">
        <span className="sr-only">Slug (optional)</span>
        <input
          name="slug"
          placeholder="slug (optional)"
          pattern="[a-z0-9]+(-[a-z0-9]+)*"
          maxLength={60}
          title="Lowercase letters, digits and single hyphens"
          className="w-full font-mono"
        />
      </label>
      <button type="submit" disabled={pending} className={buttonClass("dark")}>
        New project
      </button>
      {state?.error && (
        <span role="alert" className="w-full text-err">
          {state.error}
        </span>
      )}
    </form>
  );
}
