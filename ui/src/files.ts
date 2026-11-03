// Pure helpers for the remote file browser (POSIX paths as used by SFTP servers).

export function joinPath(dir: string, name: string): string {
  return dir === "/" ? `/${name}` : `${dir.replace(/\/+$/, "")}/${name}`;
}

export function parentOf(path: string): string {
  const p = path.replace(/\/+$/, "");
  const i = p.lastIndexOf("/");
  return i <= 0 ? "/" : p.slice(0, i);
}

export function baseName(path: string): string {
  const p = path.replace(/[\\/]+$/, "");
  return p.slice(Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")) + 1);
}

/** `/a/b` -> [{name:"/", path:"/"}, {name:"a", path:"/a"}, {name:"b", path:"/a/b"}] */
export function breadcrumbs(path: string): { name: string; path: string }[] {
  const out = [{ name: "/", path: "/" }];
  let acc = "";
  for (const seg of path.split("/").filter(Boolean)) {
    acc += `/${seg}`;
    out.push({ name: seg, path: acc });
  }
  return out;
}

export function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 10 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

/** A name that is safe to create/rename to: non-empty, no slashes, not `.`/`..`. */
export function validName(name: string): string | null {
  const n = name.trim();
  if (!n) return "Name is required";
  if (n.includes("/") || n.includes("\\")) return "Name must not contain slashes";
  if (n === "." || n === "..") return "Invalid name";
  return null;
}
