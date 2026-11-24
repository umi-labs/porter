import Link from "next/link";
import { Frame } from "@/components/porter/frame";
import { Empty } from "@/lib/view-parts";

export default function Forbidden() {
  return (
    <Frame narrow>
      <h1 className="sr-only">403</h1>
      <Empty title="You don’t have access to this page">
        403. Your role on this project doesn’t include it.{" "}
        <Link href="/" className="text-primary hover:underline">
          Back to your projects
        </Link>
      </Empty>
    </Frame>
  );
}
