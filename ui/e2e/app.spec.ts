import { expect, type Page, test } from "@playwright/test";

const screen = (page: Page) => page.evaluate(() => (window as any).__simpu.screen() as string);
const type = async (page: Page, text: string) => {
  await page.locator(".leaf.focus .xterm-helper-textarea").focus();
  await page.keyboard.type(text);
};
const open = async (page: Page, name: string, trust = true) => {
  await page.getByTestId(`conn-${name}`).dblclick();
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

  test("vault-backed connections never prompt", async ({ page }) => {
    await page.getByTestId("conn-db").dblclick();
    await expect(page.getByTestId("password-dialog")).toHaveCount(0);
    await page.getByTestId("hostkey-trust").click();
    await expect.poll(() => screen(page)).toContain("Connected");
  });
});
