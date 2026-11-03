import { expect, type Page, test } from "@playwright/test";

const screen = (page: Page) => page.evaluate(() => (window as any).__simpu.screen() as string);
const type = async (page: Page, text: string) => {
  await page.locator(".leaf.focus .xterm-helper-textarea").focus();
  await page.keyboard.type(text);
};
const open = async (page: Page, name: string, trust = true) => {
  await page.getByTestId(`conn-${name}`).dblclick();
  // The (mock) Bitwarden vault starts locked: unlock it the first time it is needed.
  // Wait for whichever dialog comes first instead of guessing a delay (slow machines).
  const first = page.locator("[data-testid=password-dialog], [data-testid=hostkey-trust]").first();
  await first.waitFor({ timeout: 5000 }).catch(() => {});
  const dialog = page.getByTestId("password-dialog");
  if (await dialog.isVisible()) {
    if ((await dialog.textContent())?.includes("Unlock")) {
      await page.getByTestId("password-input").fill("master");
      await page.getByTestId("password-ok").click();
    }
  }
  if (trust) {
    await page.getByTestId("hostkey-trust").click();
  }
};

test.beforeEach(async ({ page }) => {
  await page.goto("/");
});

test("tree lists connections and search filters", async ({ page }) => {
  await expect(page.getByTestId("conn-db")).toBeVisible();
  await expect(page.getByTestId("conn-switch")).toBeVisible();
  await page.getByTestId("search").fill("bastion");
  await expect(page.getByTestId("conn-bastion")).toBeVisible();
  await expect(page.getByTestId("conn-switch")).toHaveCount(0);
});

test("open session after trusting host key; type and see output", async ({ page }) => {
  await open(page, "db");
  await expect(page.getByTestId("tab-db")).toBeVisible();
  await expect.poll(() => screen(page)).toContain("Connected to root@db.example.com");
  await type(page, "ls -l\r");
  await expect.poll(() => screen(page)).toContain("ran: ls -l");
});

test("rejecting the host key shows a failure in the terminal", async ({ page }) => {
  await page.getByTestId("conn-db").dblclick();
  await page.getByTestId("password-input").fill("master");
  await page.getByTestId("password-ok").click();
  await page.getByTestId("hostkey-reject").click();
  await expect.poll(() => screen(page)).toContain("Connection failed");
});

test("split panes, independent sessions, close pane", async ({ page }) => {
  await open(page, "db");
  await expect.poll(() => screen(page)).toContain("Connected");
  await page.getByTestId("split-row").click();
  await expect(page.locator(".leaf")).toHaveCount(2);
  await page.getByTestId("split-col").click();
  await expect(page.locator(".leaf")).toHaveCount(3);
  await type(page, "pwd\r");
  await expect.poll(() => screen(page)).toContain("ran: pwd");
  await page.getByTestId("close-pane").click();
  await expect(page.locator(".leaf")).toHaveCount(2);
});

test("several tabs; closing a tab removes its session", async ({ page }) => {
  await open(page, "db");
  await open(page, "bastion", false); // host differs -> prompt again
  await page.getByTestId("hostkey-trust").click().catch(() => {});
  await expect(page.getByTestId("tab-bastion")).toBeVisible();
  await page.getByTestId("tab-db").click();
  await expect(page.locator(".leaf")).toHaveCount(1);
  await page.getByTestId("tab-db").getByTestId("tab-close").click();
  await expect(page.getByTestId("tab-db")).toHaveCount(0);
  await expect(page.getByTestId("tab-bastion")).toBeVisible();
});

test("telnet connections show a plaintext warning", async ({ page }) => {
  await open(page, "switch");
  await expect(page.getByTestId("banner")).toContainText("not encrypted");
});

test("dropping a file on a pane uploads it", async ({ page }) => {
  await open(page, "db");
  await expect.poll(() => screen(page)).toContain("Connected");
  const dt = await page.evaluateHandle(() => {
    const d = new DataTransfer();
    d.items.add(new File(["hello"], "report.txt"));
    return d;
  });
  await page.locator(".leaf.focus").dispatchEvent("drop", { dataTransfer: dt });
  await expect(page.getByTestId("toasts")).toContainText("Transfer complete");
  const uploads = await page.evaluate(() => (window as any).__simpu.backend.uploads);
  expect(uploads).toMatchObject([{ localPath: "report.txt", remoteDir: "." }]);
});

test("exit closes the session", async ({ page }) => {
  await open(page, "db");
  await expect.poll(() => screen(page)).toContain("Connected");
  await type(page, "exit\r");
  await expect(page.locator(".closed")).toContainText("session closed");
});

test.describe("connection editor", () => {
  test("create a connection with a vault secret and a jump host, then connect", async ({ page }) => {
    await page.getByTestId("new-conn").click();
    await page.getByTestId("edit-save").click();
    await expect(page.getByTestId("editor")).toContainText("Name is required");
    await expect(page.getByTestId("editor")).toContainText("Host is required");

    await page.getByTestId("edit-name").fill("web");
    await page.getByTestId("edit-host").fill("web.example.com");
    await page.getByTestId("edit-port").fill("99999");
    await page.getByTestId("edit-save").click();
    await expect(page.getByTestId("editor")).toContainText("Port must be 1-65535");
    await page.getByTestId("edit-port").fill("2222");
    await page.getByTestId("edit-user").fill("deploy");
    await page.getByTestId("edit-provider").selectOption("bitwarden");
    await page.getByTestId("edit-save").click();
    await expect(page.getByTestId("editor")).toContainText("Secret item is required");
    await page.getByTestId("edit-item").fill("abc-123");
    await page.getByTestId("edit-jump").selectOption({ label: "bastion" });
    await page.getByTestId("edit-folder").selectOption("prod");
    await page.getByTestId("edit-save").click();

    await expect(page.getByTestId("conn-web")).toBeVisible();
    const saved = await page.evaluate(() => (window as any).__simpu.backend.tree);
    const prod = saved.find((n: any) => n.name === "prod");
    expect(prod.children.find((c: any) => c.name === "web")).toMatchObject({
      host: "web.example.com", port: 2222, username: "deploy", jump: ["bastion"],
      secret: { provider: "bitwarden", item: "abc-123", kind: "password" },
    });
    await open(page, "web");
    await expect.poll(() => screen(page)).toContain("Connected to deploy@web.example.com");
  });

  test("edit and delete", async ({ page }) => {
    await page.getByTestId("conn-switch").hover();
    await page.getByTestId("edit-switch").click();
    await page.getByTestId("edit-host").fill("10.9.9.9");
    await page.getByTestId("edit-save").click();
    const tree = await page.evaluate(() => (window as any).__simpu.backend.tree);
    expect(JSON.stringify(tree)).toContain("10.9.9.9");

    page.once("dialog", (d) => void d.accept());
    await page.getByTestId("conn-switch").hover();
    await page.getByTestId("delete-switch").click();
    await expect(page.getByTestId("conn-switch")).toHaveCount(0);
  });

  test("new folder", async ({ page }) => {
    page.once("dialog", (d) => void d.accept("staging"));
    await page.getByTestId("new-folder").click();
    await expect(page.getByTestId("tree")).toContainText("staging");
  });
});

test.describe("ad-hoc password", () => {
  test("prompts when no vault secret is configured and uses the entered password", async ({ page }) => {
    await page.getByTestId("conn-adhoc").dblclick();
    await expect(page.getByTestId("password-dialog")).toContainText("me@adhoc.example.com");
    await page.getByTestId("password-input").fill("s3cret");
    await page.getByTestId("password-ok").click();
    await page.getByTestId("hostkey-trust").click();
    await expect.poll(() => screen(page)).toContain("Connected to me@adhoc.example.com");
    expect(await page.evaluate(() => (window as any).__simpu.backend.enteredPasswords)).toEqual(["s3cret"]);
  });

  test("cancel aborts the connection", async ({ page }) => {
    await page.getByTestId("conn-adhoc").dblclick();
    await page.getByTestId("password-cancel").click();
    await expect.poll(() => screen(page)).toContain("password entry cancelled");
  });

  test("vault-backed connections ask for the master password only, once", async ({ page }) => {
    await page.getByTestId("conn-db").dblclick();
    await expect(page.getByTestId("password-dialog")).toContainText("Unlock bitwarden");
    await page.getByTestId("password-input").fill("nope");
    await page.getByTestId("password-ok").click();
    await expect(page.getByTestId("password-error")).toContainText("Wrong master password");
    await page.getByTestId("password-input").fill("master");
    await page.getByTestId("password-ok").click();
    await page.getByTestId("hostkey-trust").click();
    await expect.poll(() => screen(page)).toContain("Connected");
    // unlocked now: a second vault-backed connection does not ask again
    await page.getByTestId("conn-bastion").dblclick();
    await page.getByTestId("hostkey-trust").click();
    await expect(page.getByTestId("password-dialog")).toHaveCount(0);
    await expect(page.getByTestId("tab-bastion")).toBeVisible();
  });
});

test("import ssh config adds a folder of hosts", async ({ page }) => {
  await page.getByTestId("import-ssh").click();
  await expect(page.getByTestId("toasts")).toContainText("Imported 2 hosts");
  await expect(page.getByTestId("conn-imported-a")).toBeVisible();
  await expect(page.getByTestId("tree")).toContainText("Imported from ssh_config");
});

test.describe("log capture", () => {
  const preview = (page: Page) => page.getByTestId("export-preview").inputValue();

  async function recordSession(page: Page) {
    await open(page, "db");
    await expect.poll(() => screen(page)).toContain("Connected");
    await page.getByTestId("rec").click();
    await expect(page.locator(".leaf.recording")).toHaveCount(1);
    await type(page, "\r"); // an empty line so the log starts at a prompt
    await expect.poll(() => screen(page)).toContain("ran: ");
    await type(page, "uname -a\r");
    await expect.poll(() => screen(page)).toContain("ran: uname -a");
    await type(page, "ssh db.example.com\r");
    await expect.poll(() => screen(page)).toContain("ran: ssh db.example.com");
    await type(page, "echo hi root\r");
    await expect.poll(() => screen(page)).toContain("ran: echo hi root");
  }

  test("record, stop, export everything or the last command", async ({ page }) => {
    await recordSession(page);
    await page.getByTestId("rec").click(); // stop
    await expect(page.locator(".leaf.recording")).toHaveCount(0);
    await page.getByTestId("export-log").click();
    await expect.poll(() => preview(page)).toContain("ran: uname -a");

    await page.getByTestId("scope-last").check();
    await expect.poll(() => preview(page)).not.toContain("uname");
    expect(await preview(page)).toContain("echo hi root");
    await page.getByTestId("last-n").fill("3");
    await expect.poll(() => preview(page)).toContain("uname -a");
  });

  test("anonymize usernames and hosts with preview, custom regex and validation", async ({ page }) => {
    await recordSession(page);
    await page.getByTestId("export-log").click();
    await page.getByTestId("anon-users").check();
    await page.getByTestId("anon-hosts").check();
    await expect.poll(() => preview(page)).toContain("ssh host1");
    expect(await preview(page)).not.toContain("db.example.com");
    expect(await preview(page)).toContain("echo hi user1");
    await expect(page.getByTestId("export-map")).toContainText("db.example.com → host1");

    await page.getByTestId("anon-custom").fill("hi (root");
    await expect(page.getByTestId("regex-errors")).not.toBeEmpty();
    await page.getByTestId("anon-custom").fill("hi \\w+");
    await expect(page.getByTestId("regex-errors")).toBeEmpty();
    await expect.poll(() => preview(page)).toContain("echo <rule1-1>");
  });

  test("copy to clipboard and save to file export the anonymized text", async ({ page }) => {
    await recordSession(page);
    await page.getByTestId("export-log").click();
    await page.getByTestId("anon-users").check();
    await expect.poll(() => preview(page)).toContain("user1");
    await page.getByTestId("export-copy").click();
    await expect(page.getByTestId("toasts")).toContainText("Copied");
    await page.getByTestId("export-save").click();
    await expect(page.getByTestId("toasts")).toContainText("Saved /tmp/session.log");
    const b = await page.evaluate(() => (window as any).__simpu.backend);
    expect(b.clipboard).toContain("user1");
    expect(b.clipboard).not.toContain("root@");
    expect(b.savedFiles).toHaveLength(1);
    expect(b.savedFiles[0].text).toBe(b.clipboard);
  });

  test("exporting without recording shows a clear message", async ({ page }) => {
    await open(page, "db");
    await expect.poll(() => screen(page)).toContain("Connected");
    await page.getByTestId("export-log").click();
    await expect(page.getByTestId("export-note")).toContainText("no capture");
  });
});

test("each vault provider is unlocked separately", async ({ page }) => {
  await page.getByTestId("new-conn").click();
  await page.getByTestId("edit-name").fill("kp");
  await page.getByTestId("edit-host").fill("kp.example.com");
  await page.getByTestId("edit-provider").selectOption("keepassxc");
  await page.getByTestId("edit-item").fill("Servers/kp");
  await page.getByTestId("edit-save").click();
  await open(page, "db"); // unlocks bitwarden (helper)
  await page.getByTestId("conn-kp").dblclick();
  await expect(page.getByTestId("password-dialog")).toContainText("Unlock keepassxc");
  await page.getByTestId("password-input").fill("master");
  await page.getByTestId("password-ok").click();
  await page.getByTestId("hostkey-trust").click();
  await expect(page.getByTestId("tab-kp")).toBeVisible();
});

test.describe("forwards and reconnect", () => {
  const mockOf = (page: Page) => page.evaluate(() => (window as any).__simpu.backend);

  async function newConn(page: Page, extra: { forwards?: string; reconnect?: string; noShell?: boolean } = {}) {
    await page.getByTestId("new-conn").click();
    await page.getByTestId("edit-name").fill("tun");
    await page.getByTestId("edit-host").fill("tun.example.com");
    await page.getByTestId("edit-provider").selectOption("file");
    await page.getByTestId("edit-item").fill("~/.ssh/id_ed25519");
    if (extra.forwards) await page.getByTestId("edit-forwards").fill(extra.forwards);
    if (extra.reconnect) await page.getByTestId("edit-reconnect").selectOption(extra.reconnect);
    if (extra.noShell) await page.getByTestId("edit-noshell").check();
    await page.getByTestId("edit-save").click();
  }

  async function openTun(page: Page) {
    await page.getByTestId("conn-tun").dblclick();
    await page.getByTestId("hostkey-trust").click();
    await expect.poll(() => screen(page)).toContain("Connected");
  }

  test("editor saves forwards, policy and tunnel-only; validates syntax; warns about exposed binds", async ({ page }) => {
    await page.getByTestId("new-conn").click();
    await page.getByTestId("edit-name").fill("tun");
    await page.getByTestId("edit-host").fill("h");
    await page.getByTestId("edit-forwards").fill("L 5433:db:5432\nbogus line");
    await page.getByTestId("edit-save").click();
    await expect(page.getByTestId("edit-forwards-err")).toContainText("line 2");
    await page.getByTestId("edit-forwards").fill("L 5433:db:5432\nD 0.0.0.0:1080");
    await expect(page.getByTestId("edit-forwards-warn")).toContainText("non-loopback");
    await page.getByTestId("edit-reconnect").selectOption("off");
    await page.getByTestId("edit-noshell").check();
    await page.getByTestId("edit-save").click();
    const tree = await page.evaluate(() => (window as any).__simpu.backend.tree);
    const c = tree.find((n: any) => n.name === "tun");
    expect(c).toMatchObject({
      reconnect: "off",
      no_shell: true,
      forwards: [
        { kind: "local", bind_port: 5433, dest_host: "db", dest_port: 5432 },
        { kind: "dynamic", bind_host: "0.0.0.0", bind_port: 1080 },
      ],
    });
    // editing shows the saved text again
    await page.getByTestId("conn-tun").hover();
    await page.getByTestId("edit-tun").click();
    await expect(page.getByTestId("edit-forwards")).toHaveValue("L 5433:db:5432\nD 0.0.0.0:1080");
    await expect(page.getByTestId("edit-reconnect")).toHaveValue("off");
  });

  test("saved forwards appear in the panel; add and stop ad-hoc forwards", async ({ page }) => {
    await newConn(page, { forwards: "L 5433:db:5432" });
    await openTun(page);
    await page.getByTestId("forwards-btn").click();
    await expect(page.getByTestId("forwards-rows")).toContainText("5433");
    await expect(page.getByTestId("fw-status").first()).toContainText("listening");

    await page.getByTestId("fw-kind").selectOption("dynamic");
    await page.getByTestId("fw-port").fill("1080");
    await page.getByTestId("fw-add").click();
    await expect(page.getByTestId("forwards-rows")).toContainText("SOCKS5 proxy");

    await page.getByTestId("fw-port").fill("99999");
    await page.getByTestId("fw-add").click();
    await expect(page.getByTestId("fw-error")).toContainText("1-65535");

    await page.getByTestId("fw-stop-1").click();
    await expect(page.getByTestId("forwards-rows")).not.toContainText("db:5432");
    await page.getByTestId("fw-close").click();
  });

  test("auto reconnect: banner while reconnecting, then back; forwards still listed", async ({ page }) => {
    await newConn(page, { forwards: "L 5433:db:5432" }); // default policy with forwards = auto
    await openTun(page);
    const sid = await page.evaluate(() => [...(window as any).__simpu.backend.sessions.keys()][0]);
    await page.evaluate((id) => {
      const b = (window as any).__simpu.backend;
      b.reconnectDelayMs = 800;
      b.dropSession(id);
    }, sid);
    await expect(page.getByTestId("status-text")).toContainText("Reconnecting");
    await expect.poll(() => screen(page)).toContain("[connection lost]");
    await expect(page.getByTestId("statusbar")).toBeHidden({ timeout: 5000 });
    await expect.poll(() => screen(page)).toContain("[reconnected]");
    await page.getByTestId("forwards-btn").click();
    await expect(page.getByTestId("forwards-rows")).toContainText("5433");
  });

  test("ask policy: lost banner with Reconnect and Close buttons", async ({ page }) => {
    await newConn(page); // no forwards -> ask
    await openTun(page);
    const sid = await page.evaluate(() => [...(window as any).__simpu.backend.sessions.keys()][0]);
    await page.evaluate((id) => (window as any).__simpu.backend.dropSession(id), sid);
    await expect(page.getByTestId("status-text")).toContainText("Connection lost");
    await page.waitForTimeout(500);
    expect((await mockOf(page).then((b) => b)).reconnects).toEqual([]);
    await page.getByTestId("reconnect").click();
    await expect(page.getByTestId("statusbar")).toBeHidden();
    expect(await page.evaluate(() => (window as any).__simpu.backend.reconnects.length)).toBe(1);

    // drop again and choose Close
    await page.evaluate((id) => (window as any).__simpu.backend.dropSession(id), sid);
    await page.getByTestId("status-close").click();
    await expect(page.locator(".leaf")).toHaveCount(0);
  });

  test("off policy closes the session", async ({ page }) => {
    await newConn(page, { reconnect: "off" });
    await openTun(page);
    const sid = await page.evaluate(() => [...(window as any).__simpu.backend.sessions.keys()][0]);
    await page.evaluate((id) => (window as any).__simpu.backend.dropSession(id), sid);
    await expect(page.locator(".closed")).toContainText("connection lost");
    await expect(page.getByTestId("statusbar")).toBeHidden();
  });
});

test.describe("keys and agent", () => {
  async function create(page: Page, provider: string, item: string) {
    await page.getByTestId("new-conn").click();
    await page.getByTestId("edit-name").fill("keyed");
    await page.getByTestId("edit-host").fill("keyed.example.com");
    await page.getByTestId("edit-provider").selectOption(provider);
    if (item) await page.getByTestId("edit-item").fill(item);
    await page.getByTestId("edit-save").click();
  }

  test("ssh-agent needs no item and connects without prompts", async ({ page }) => {
    await create(page, "agent", "");
    const tree = await page.evaluate(() => (window as any).__simpu.backend.tree);
    expect(tree.find((n: any) => n.name === "keyed").secret).toMatchObject({ provider: "agent", item: "" });
    await page.getByTestId("conn-keyed").dblclick();
    await page.getByTestId("hostkey-trust").click();
    await expect.poll(() => screen(page)).toContain("Connected to");
    await expect(page.getByTestId("password-dialog")).toHaveCount(0);
  });

  test("encrypted key file asks for the passphrase, retries when wrong", async ({ page }) => {
    await create(page, "file", "~/.ssh/id_encrypted");
    await page.getByTestId("conn-keyed").dblclick();
    await expect(page.getByTestId("password-dialog")).toContainText("Key passphrase required");
    await page.getByTestId("password-input").fill("nope");
    await page.getByTestId("password-ok").click();
    await expect(page.getByTestId("password-error")).toContainText("Wrong passphrase");
    await page.getByTestId("password-input").fill("pass");
    await page.getByTestId("password-ok").click();
    await page.getByTestId("hostkey-trust").click();
    await expect.poll(() => screen(page)).toContain("Connected");
  });
});

test.describe("remote file browser", () => {
  const backendOf = (page: Page) => page.evaluate(() => (window as any).__simpu.backend);

  async function openFiles(page: Page) {
    await open(page, "db");
    await expect.poll(() => screen(page)).toContain("Connected");
    await page.getByTestId("files-btn").click();
    await expect(page.getByTestId("files-path")).toContainText("home");
  }

  test("starts in the home directory with folders first; navigate down, up and via breadcrumbs", async ({ page }) => {
    await openFiles(page);
    await expect(page.getByTestId("files-path")).toHaveText(/user/);
    const rows = page.locator("[data-testid^=file-row-]");
    await expect(rows).toHaveCount(2);
    await expect(rows.first()).toHaveAttribute("data-testid", "file-row-logs");

    await page.getByTestId("file-row-logs").dblclick();
    await expect(page.getByTestId("file-row-app.log")).toBeVisible();
    await expect(page.getByTestId("files-path")).toContainText("logs");
    await page.getByTestId("file-up").click();
    await expect(page.getByTestId("file-row-notes.txt")).toBeVisible();
    await page.getByTestId("files-path").getByRole("button", { name: "home" }).click();
    await expect(page.getByTestId("file-row-user")).toBeVisible();
  });

  test("create, rename and delete; invalid names and non-empty folders are refused", async ({ page }) => {
    await openFiles(page);
    page.once("dialog", (d) => void d.accept("backup"));
    await page.getByTestId("file-mkdir").click();
    await expect(page.getByTestId("file-row-backup")).toBeVisible();

    page.once("dialog", (d) => void d.accept("a/b"));
    await page.getByTestId("file-mkdir").click();
    await expect(page.getByTestId("toasts")).toContainText("slashes");

    page.once("dialog", (d) => void d.accept("readme.txt"));
    await page.getByTestId("file-row-notes.txt").hover();
    await page.getByTestId("file-rename-notes.txt").click();
    await expect(page.getByTestId("file-row-readme.txt")).toBeVisible();
    await expect(page.getByTestId("file-row-notes.txt")).toHaveCount(0);

    page.once("dialog", (d) => void d.accept());
    await page.getByTestId("file-row-readme.txt").hover();
    await page.getByTestId("file-delete-readme.txt").click();
    await expect(page.getByTestId("file-row-readme.txt")).toHaveCount(0);

    page.once("dialog", (d) => void d.accept());
    await page.getByTestId("file-row-logs").hover();
    await page.getByTestId("file-delete-logs").click();
    await expect(page.getByTestId("toasts")).toContainText("not empty");
    await expect(page.getByTestId("file-row-logs")).toBeVisible();
  });

  test("upload by dialog and by drop, download to a chosen folder", async ({ page }) => {
    await openFiles(page);
    await page.getByTestId("file-upload").click();
    await expect(page.getByTestId("file-row-report.pdf")).toBeVisible();
    expect((await backendOf(page)).uploads.at(-1)).toMatchObject({ localPath: "/tmp/report.pdf", remoteDir: "/home/user" });

    const dt = await page.evaluateHandle(() => {
      const d = new DataTransfer();
      d.items.add(new File(["x"], "dropped.txt"));
      return d;
    });
    await page.getByTestId("files").dispatchEvent("drop", { dataTransfer: dt });
    await expect(page.getByTestId("file-row-dropped.txt")).toBeVisible();

    await page.getByTestId("file-row-notes.txt").hover();
    await page.getByTestId("file-download-notes.txt").click();
    await expect.poll(async () => (await backendOf(page)).downloads.length).toBe(1);
    expect((await backendOf(page)).downloads[0]).toMatchObject({ remotePath: "/home/user/notes.txt", localDir: "/tmp/downloads" });
  });

  test("a cancelled folder dialog downloads nothing; closing the panel hides it", async ({ page }) => {
    await openFiles(page);
    await page.evaluate(() => ((window as any).__simpu.backend.nextFolder = null));
    await page.getByTestId("file-row-notes.txt").dblclick();
    await page.waitForTimeout(300);
    expect((await backendOf(page)).downloads).toEqual([]);
    await page.getByTestId("files-btn").click();
    await expect(page.getByTestId("files")).toBeHidden();
  });

  test("without a session the panel explains what to do", async ({ page }) => {
    await page.getByTestId("files-btn").click();
    await expect(page.getByTestId("files-error")).toContainText("Open a session");
  });
});
