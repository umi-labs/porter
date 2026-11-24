"use client";
import { useActionState } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { FieldError } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { createProject } from "./actions";

/** Name, and an optional slug (made from the name when left empty). */
export function NewProjectForm() {
  const [state, action, pending] = useActionState<{ error?: string } | null, FormData>(createProject, null);
  return (
    <Card className="mt-6">
      <CardHeader>
        <CardTitle className="text-sm font-semibold">New project</CardTitle>
        <CardDescription>
          Then run <code>porter hub link &lt;slug&gt;</code> in the porter project.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form action={action} className="flex flex-wrap items-center gap-2" aria-label="New project">
          <div className="flex min-w-0 flex-[2_1_220px]">
            <Input
              aria-label="Project name"
              name="name"
              placeholder="Project name"
              required
              maxLength={120}
              className="h-9"
            />
          </div>
          <div className="flex min-w-0 flex-[1_1_160px]">
            <Input
              aria-label="Slug (optional)"
              name="slug"
              placeholder="slug (optional)"
              pattern="[a-z0-9]+(-[a-z0-9]+)*"
              maxLength={60}
              title="Lowercase letters, digits and single hyphens"
              className="h-9 font-mono"
            />
          </div>
          <Button type="submit" size="lg" disabled={pending} className="press h-9 px-4">
            New project
          </Button>
          {state?.error && <FieldError className="w-full">{state.error}</FieldError>}
        </form>
      </CardContent>
    </Card>
  );
}
