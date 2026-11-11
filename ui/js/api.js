// Talking to the Rust side: `POST /api/<command>` with a JSON body, answered with
// `{"ok": value}` or `{"error": "message for the user"}`.

/** An error whose message is written for the person using the app. */
export class ApiError extends Error {}

async function send(command, args) {
  let answer;
  try {
    const response = await fetch(`/api/${command}`, { method: "POST", body: JSON.stringify(args ?? null) });
    answer = await response.json();
  } catch {
    throw new ApiError("The window lost contact with Vault Key. Close the app and start it again.");
  }
  if (answer && typeof answer === "object" && "error" in answer) throw new ApiError(answer.error);
  return answer?.ok ?? null;
}

let transport = send;

/** Replace the transport (the mock API in `ui/dev` does this when the page runs in a browser). */
export function setTransport(fn) {
  transport = fn;
}

/** Run one command. Resolves with its result, or rejects with an `ApiError`. */
export function call(command, args) {
  return transport(command, args);
}
