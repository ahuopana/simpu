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
