import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ErrorBoundary } from "../../components/ErrorBoundary";
import { IpcError } from "../../ipc/call";
import type { AppError, ErrorReport } from "../../ipc/types";
import { setBackend } from "../../ipc";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { A } from "../../lib/actionIds";
import { guard, reportError } from "../../lib/reportError";
import { setupApp } from "../../test/harness";
import { resetStore, useStore } from "../../state/store";
import { quickTarget, sshConfigTarget } from "../../lib/targets";
import { runAction } from "../actions/registry";
import { openSession } from "../session/sessionActions";
import { ErrorReportHost } from "./ErrorReportDialog";
import { installGlobalErrorHandlers, openErrorDetails, sourceFromAppError } from "./errorReport";

// Hostile characters are built from code points: no invisible character sits in this source.
const RLO = String.fromCodePoint(0x202e);
const ZWSP = String.fromCodePoint(0x200b);
const VS15 = String.fromCodePoint(0xfe0e);
const NL = String.fromCodePoint(10);
const HOSTILE = ["## Error", "<img src=x onerror=alert(1)>", `<b>bold</b> [link](javascript:alert(1)) evil${RLO}txt${ZWSP}${VS15}`, "```", ""].join(NL);

const BUG: AppError = { code: "bug", params: { command: "sftp_list" }, detail: "panicked at src/commands/sftp.rs:1:1" };
const INTERNAL: AppError = { code: "internal", detail: "boom" };
const LOG_LABEL = "Include the last lines of the log";
const PREVIEW_LABEL = "Report (exactly what is copied)";

function stubClipboard(fail = false) {
  const writeText = vi.fn(() => (fail ? Promise.reject(new Error("denied")) : Promise.resolve()));
  Object.defineProperty(navigator, "clipboard", { value: { writeText, readText: vi.fn() }, configurable: true });
  return writeText;
}

const dialog = () => screen.queryByRole("dialog", { name: /Error details|Report a problem/ });
const preview = () => screen.getByLabelText(PREVIEW_LABEL);

function Thrower(): never {
  throw new Error("render crash");
}

describe("error details dialog", () => {
  beforeAll(() => installGlobalErrorHandlers());
  beforeEach(() => vi.spyOn(console, "error").mockImplementation(() => undefined));
  afterEach(() => vi.restoreAllMocks());

  it("Help > Report a problem opens it without an error: environment only, log off by default", async () => {
    const { backend, user } = setupApp();
    await user.click(screen.getByRole("menuitem", { name: "Help" }));
    await user.click(await screen.findByRole("menuitem", { name: /Report a problem…/ }));
    const d = await screen.findByRole("dialog", { name: "Report a problem" });
    await waitFor(() => expect(preview().textContent).toContain("Environment"));
    expect(backend.errorReportInputs[0]).toMatchObject({ code: null, message: null, includeLog: false });
    expect((within(d).getByRole("checkbox", { name: LOG_LABEL }) as HTMLInputElement).checked).toBe(false);
    expect(preview().textContent).not.toContain("mock log line");
  });

  it("shows the preview as literal text (markup, bidi and invisible characters are not interpreted)", async () => {
    const { backend } = setupApp();
    const report: ErrorReport = {
      markdown: HOSTILE,
      issueUrl: "https://github.com/shellfin-app/shellfin/issues/new?template=bug_report.yml",
      environment: { appVersion: "0.1.0", os: "x", arch: "x86_64", install: null, uiLanguage: null },
      logIncluded: false,
      logLines: 0,
      logLinesInUrl: 0,
    };
    vi.spyOn(backend, "appErrorReport").mockResolvedValue(report);
    act(() => openErrorDetails(null));
    await waitFor(() => expect(preview().textContent).toBe(HOSTILE));
    expect(preview().querySelector("img, b, a")).toBeNull();
    expect(preview().tagName).toBe("PRE");
  });

  it("toggling the log checkbox re-requests the report with includeLog and notes lines that do not fit the URL", async () => {
    const { backend, user } = setupApp();
    act(() => openErrorDetails(null));
    await waitFor(() => expect(backend.errorReportInputs).toHaveLength(1));
    await user.click(await screen.findByRole("checkbox", { name: LOG_LABEL }));
    await waitFor(() => expect(backend.errorReportInputs).toHaveLength(2));
    expect(backend.errorReportInputs[1]).toMatchObject({ includeLog: true });
    await waitFor(() => expect(preview().textContent).toContain("mock log line 60"));
    expect(screen.getByText(/of 60 log lines fit into the form/)).toBeTruthy();
    await user.click(screen.getByRole("checkbox", { name: LOG_LABEL }));
    await waitFor(() => expect(preview().textContent).not.toContain("mock log line"));
  });

  it("Report on GitHub copies the report, then opens the exact issueUrl from the backend", async () => {
    const { backend, user } = setupApp();
    const writeText = stubClipboard();
    const open = vi.spyOn(backend, "appOpenExternal").mockResolvedValue(true);
    const report: ErrorReport = {
      markdown: HOSTILE,
      issueUrl: "https://github.com/shellfin-app/shellfin/issues/new?template=bug_report.yml&title=exact-from-backend",
      environment: { appVersion: "0.1.0", os: "x", arch: "x86_64", install: null, uiLanguage: null },
      logIncluded: false,
      logLines: 0,
      logLinesInUrl: 0,
    };
    vi.spyOn(backend, "appErrorReport").mockResolvedValue(report);
    act(() => openErrorDetails(sourceFromAppError(BUG)));
    const button = await screen.findByRole("button", { name: "Report on GitHub…" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    await user.click(button);
    await waitFor(() => expect(open).toHaveBeenCalledTimes(1));
    expect(writeText).toHaveBeenCalledWith(HOSTILE);
    expect((open.mock.calls[0] as unknown[] | undefined)?.[0]).toBe(report.issueUrl);
    expect(writeText.mock.invocationCallOrder[0]).toBeLessThan(open.mock.invocationCallOrder[0] ?? 0);
    expect(await screen.findByText(/full report is on the clipboard/)).toBeTruthy();
  });

  it("the mock models the URL limit", async () => {
    const backend = new MockBackend();
    const report = await backend.appErrorReport({ includeLog: true, code: "bug" });
    expect(report.issueUrl.length).toBeLessThanOrEqual(2048);
    expect(report.issueUrl.startsWith("https://github.com/shellfin-app/shellfin/issues/new?template=bug_report.yml")).toBe(true);
    expect(report.logIncluded).toBe(true);
    expect(report.logLinesInUrl).toBeLessThan(report.logLines);
    const without = await backend.appErrorReport({ includeLog: false });
    expect(without).toMatchObject({ logIncluded: false, logLines: 0, logLinesInUrl: 0 });
  });

  it("a clipboard failure still opens the form and tells the user to use Copy details", async () => {
    const { backend, user } = setupApp();
    stubClipboard(true);
    const open = vi.spyOn(backend, "appOpenExternal").mockResolvedValue(true);
    act(() => openErrorDetails(sourceFromAppError(BUG)));
    const button = await screen.findByRole("button", { name: "Report on GitHub…" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    await user.click(button);
    await waitFor(() => expect(open).toHaveBeenCalledTimes(1));
    expect(await screen.findByText(/could not be copied to the clipboard/)).toBeTruthy();
    expect(screen.queryByText(/full report is on the clipboard/)).toBeNull();
  });

  it("the report request failing with a bug is shown in the dialog and not counted as another error", async () => {
    const { backend } = setupApp();
    vi.spyOn(backend, "appErrorReport").mockRejectedValue(new IpcError(BUG));
    act(() => openErrorDetails(null));
    expect(await screen.findByText("The report could not be prepared.")).toBeTruthy();
    expect(screen.queryByText(/more unexpected error/)).toBeNull();
    expect(useStore.getState().notices).toHaveLength(0);
    expect(useStore.getState().dialog).toMatchObject({ kind: "errorReport", more: 0 });
  });

  it("Details… on a notice reports when the error happened, not the click time", async () => {
    const { backend, user } = setupApp();
    const pushed = Date.now() - 60_000;
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(pushed);
    act(() => reportError(new IpcError({ code: "connectRefused", params: { host: "h", port: 22 } })));
    vi.setSystemTime(pushed + 30_000);
    await user.click(within(await screen.findByRole("alert")).getByRole("button", { name: "Details…" }));
    vi.useRealTimers();
    await waitFor(() => expect(backend.errorReportInputs[0]?.occurredAtMs).toBe(pushed));
  });

  it("a render crash with a dialog open still shows the report (the stale dialog is replaced)", async () => {
    resetStore();
    setBackend(new MockBackend());
    act(() => useStore.getState().openDialog({ kind: "about" }));
    render(
      <ErrorBoundary reportHost>
        <Thrower />
      </ErrorBoundary>,
    );
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(useStore.getState().dialog?.kind).toBe("errorReport");
    expect(useStore.getState().notices).toHaveLength(0);
  });

  it("a render crash with the palette open closes it and shows the report", async () => {
    resetStore();
    setBackend(new MockBackend());
    act(() => useStore.getState().openPalette());
    render(
      <ErrorBoundary reportHost>
        <Thrower />
      </ErrorBoundary>,
    );
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(useStore.getState().palette).toBeNull();
  });

  it("the connection failure panel has Details… (and keeps its own Copy details)", async () => {
    const { user, backend } = setupApp();
    useStore.getState().addSession("s1", quickTarget("root@h:22"), "h");
    act(() =>
      useStore.getState().applySessionEvent("s1", {
        type: "state",
        state: { phase: "disconnected", error: { code: "connectRefused", params: { host: "h", port: 22 } }, canRetry: true },
      }),
    );
    const panel = await screen.findByRole("alert");
    expect(within(panel).getByRole("button", { name: "Copy details" })).toBeTruthy();
    await user.click(within(panel).getByRole("button", { name: "Details…" }));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    await waitFor(() => expect(backend.errorReportInputs[0]).toMatchObject({ code: "connectRefused" }));
  });

  it("an internal failure delivered by a session event opens the dialog (bugReport scenario)", async () => {
    setupApp({ scenario: "bugReport" });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(screen.getAllByText("An internal error occurred.").length).toBeGreaterThan(0);
  });

  it("Copy details puts the previewed text on the clipboard", async () => {
    const { user } = setupApp();
    const writeText = stubClipboard();
    act(() => openErrorDetails(sourceFromAppError(BUG)));
    const button = await screen.findByRole("button", { name: "Copy details" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    await user.click(button);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(preview().textContent));
  });

  it("a failing report request is reported and disables the actions", async () => {
    const { backend } = setupApp();
    vi.spyOn(backend, "appErrorReport").mockRejectedValue(new IpcError({ code: "storageReadOnly" }));
    act(() => openErrorDetails(null));
    expect(await screen.findByText("The report could not be prepared.")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Report on GitHub…" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Copy details" }) as HTMLButtonElement).disabled).toBe(true);
    expect(useStore.getState().notices).toHaveLength(1);
  });

  it("opens by itself for backend internal errors, without a toast", async () => {
    setupApp();
    act(() => reportError(new IpcError(INTERNAL)));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(screen.getAllByText("An internal error occurred.").length).toBeGreaterThan(0);
    expect(useStore.getState().notices).toHaveLength(0);
  });

  it("opens by itself for bug: the message names the command and the report gets it as the context", async () => {
    const { backend } = setupApp();
    act(() => reportError(new IpcError(BUG)));
    expect(await screen.findByText(/Something went wrong inside Shellfin \(sftp_list\)/)).toBeTruthy();
    await waitFor(() => expect(backend.errorReportInputs[0]).toMatchObject({ code: "bug", context: "sftp_list", includeLog: false }));
  });

  it("does not open for expected errors: they keep the notice, which offers Details…", async () => {
    const { user } = setupApp();
    act(() => reportError(new IpcError({ code: "connectRefused", params: { host: "h", port: 22 } })));
    expect(dialog()).toBeNull();
    const notice = await screen.findByRole("alert");
    await user.click(within(notice).getByRole("button", { name: "Details…" }));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(useStore.getState().notices).toHaveLength(0);
    expect(screen.getByText(/Code: connectRefused/)).toBeTruthy();
  });

  it("a rejection that is not from the backend keeps the notice (no dialog)", () => {
    setupApp();
    act(() => reportError(new Error("clipboard denied")));
    expect(dialog()).toBeNull();
    expect(useStore.getState().notices).toHaveLength(1);
  });

  it("repeats while it is open only increase a counter", async () => {
    setupApp();
    act(() => reportError(new IpcError(INTERNAL)));
    await screen.findByRole("dialog", { name: "Error details" });
    act(() => reportError(new IpcError(INTERNAL)));
    act(() => reportError(new IpcError(BUG)));
    expect(await screen.findByText("2 more unexpected errors happened since this dialog opened.")).toBeTruthy();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
  });

  it("does not replace another open dialog: a notice with Details… is queued instead", async () => {
    setupApp();
    act(() => useStore.getState().openDialog({ kind: "about" }));
    await screen.findByRole("dialog", { name: "About Shellfin" });
    act(() => reportError(new IpcError(INTERNAL)));
    expect(useStore.getState().dialog?.kind).toBe("about");
    expect(within(await screen.findByRole("alert")).getByRole("button", { name: "Details…" })).toBeTruthy();
  });

  it("an unhandled rejection and a window error open it; ResizeObserver noise does not", async () => {
    setupApp();
    const rejection = Object.assign(new Event("unhandledrejection"), { reason: new Error("late failure") });
    act(() => void window.dispatchEvent(rejection));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(screen.getByText("Something went wrong in the Shellfin window. This is a bug; please report it.")).toBeTruthy();
    expect(screen.getByText(/Where: unhandledrejection/)).toBeTruthy();
    act(() => useStore.getState().closeDialog());
    act(() => void window.dispatchEvent(new ErrorEvent("error", { message: "ResizeObserver loop completed with undelivered notifications." })));
    expect(dialog()).toBeNull();
    act(() => void window.dispatchEvent(new ErrorEvent("error", { message: "x is not a function", error: new TypeError("x is not a function") })));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(screen.getByText(/Where: window.error/)).toBeTruthy();
  });

  it("an unhandled backend rejection opens it, a cancelled one does not", async () => {
    setupApp();
    act(() => void window.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason: new IpcError({ code: "cancelled" }) })));
    expect(dialog()).toBeNull();
    act(() => void window.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason: new IpcError({ code: "promptTimeout" }) })));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
  });

  it("guard() routes a rejected promise through the same path", async () => {
    setupApp();
    guard(Promise.reject(new IpcError(INTERNAL)));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
  });

  it("an ErrorBoundary crash keeps the Retry panel and opens the dialog beside it", async () => {
    resetStore();
    setBackend(new MockBackend());
    render(
      <ErrorBoundary reportHost>
        <Thrower />
      </ErrorBoundary>,
    );
    expect(screen.getByText("This view failed to load.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(screen.getByText(/Where: ErrorBoundary/)).toBeTruthy();
  });

  it("the crash panel's own button reopens the dialog after it was closed", async () => {
    resetStore();
    setBackend(new MockBackend());
    const user = userEvent.setup();
    render(
      <ErrorBoundary reportHost>
        <Thrower />
      </ErrorBoundary>,
    );
    await screen.findByRole("dialog", { name: "Error details" });
    act(() => useStore.getState().closeDialog());
    expect(dialog()).toBeNull();
    await user.click(screen.getByRole("button", { name: "Report this error…" }));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
  });

  it("the bugReport scenario: Help > Open logs folder fails with bug and opens the dialog", async () => {
    const { user } = setupApp({ scenario: "bugReport" });
    await user.click(screen.getByRole("menuitem", { name: "Help" }));
    await user.click(await screen.findByRole("menuitem", { name: "Open logs folder" }));
    expect(await screen.findByRole("dialog", { name: "Error details" })).toBeTruthy();
    expect(screen.getAllByText(/app_open_logs_dir/).length).toBeGreaterThan(0);
  });

  it("the host renders nothing while no such dialog is open", () => {
    resetStore();
    const { container } = render(<ErrorReportHost />);
    expect(container.textContent).toBe("");
  });

  it("runAction(reportProblem) is the Help item's behaviour", async () => {
    setupApp();
    act(() => runAction(A.reportProblem));
    expect(await screen.findByRole("dialog", { name: "Report a problem" })).toBeTruthy();
  });
});
