import { Empty } from "../../../lib/view-parts";

/** Unknown, expired and revoked links look the same: no project name, no reason. */
export default function ShareNotFound() {
  return (
    <Empty title="This link doesn’t work">
      It may have expired or been turned off. Ask the person who sent it for a new one.
    </Empty>
  );
}
