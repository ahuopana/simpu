import { describe, expect, it } from "vitest";
import { baseName, breadcrumbs, formatSize, joinPath, parentOf, validName } from "../src/files";

describe("remote paths", () => {
  it("joins and finds parents", () => {
    expect(joinPath("/", "a")).toBe("/a");
    expect(joinPath("/home/u", "x.txt")).toBe("/home/u/x.txt");
    expect(joinPath("/home/u/", "x")).toBe("/home/u/x");
    expect(parentOf("/home/u")).toBe("/home");
    expect(parentOf("/home")).toBe("/");
    expect(parentOf("/")).toBe("/");
    expect(parentOf("/a/b/")).toBe("/a");
  });

  it("base names work for posix and windows local paths", () => {
    expect(baseName("/a/b/c.txt")).toBe("c.txt");
    expect(baseName("C:\\Users\\me\\report.pdf")).toBe("report.pdf");
    expect(baseName("plain")).toBe("plain");
  });

  it("builds breadcrumbs", () => {
    expect(breadcrumbs("/")).toEqual([{ name: "/", path: "/" }]);
    expect(breadcrumbs("/a/b")).toEqual([
      { name: "/", path: "/" },
      { name: "a", path: "/a" },
      { name: "b", path: "/a/b" },
    ]);
  });

  it("formats sizes", () => {
    expect([0, 1023, 1024, 1536, 10 * 1024 * 1024, 5 * 1024 ** 3].map(formatSize)).toEqual(["0 B", "1023 B", "1.0 KB", "1.5 KB", "10 MB", "5.0 GB"]);
  });

  it("validates names", () => {
    expect(validName("ok.txt")).toBeNull();
    expect(validName(" ")).not.toBeNull();
    expect(validName("a/b")).not.toBeNull();
    expect(validName("..")).not.toBeNull();
  });
});
