// Thin wrappers over the Rust commands (src-tauri/src/lib.rs, library.rs).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const api = {
  // connection
  listSerialPorts: () => invoke("list_serial_ports"),
  discoverWifi: () => invoke("discover_wifi"),
  connectUsb: (port) => invoke("connect_usb", { port }),
  connectWifi: (host, wsPort = null) => invoke("connect_wifi", { host, wsPort }),
  disconnect: () => invoke("disconnect"),
  status: () => invoke("device_status"),
  // files
  scan: () => invoke("scan_device"),
  listDir: (path) => invoke("list_dir", { path }),
  makeDir: (path) => invoke("make_dir", { path }),
  remove: (path) => invoke("remove_path", { path }),
  rename: (from, to) => invoke("rename_path", { from, to }),
  /** items: [[id, localPath], …] */
  upload: (items, destDir, overwrite) => invoke("upload_files", { items, destDir, overwrite }),
  download: (path, saveTo) => invoke("download_file", { path, saveTo }),
  // books and images
  bookInfo: (path, size) => invoke("book_info", { path, size }),
  bookCover: (path, size) => invoke("book_cover", { path, size }),
  imageThumb: (path, size) => invoke("image_thumb", { path, size }),
  loadLocalImage: (localPath) => invoke("load_local_image", { localPath }),
  saveBook: (path, size, input) => invoke("save_book", { path, size, input }),
  exportBook: (path, size, input, saveTo) => invoke("export_book", { path, size, input, saveTo }),
  onTransfer: (cb) => listen("transfer", (e) => cb(e.payload)),
};

export function joinPath(dir, name) {
  return dir === "/" ? `/${name}` : `${dir.replace(/\/+$/, "")}/${name}`;
}

export function parentPath(path) {
  const i = path.replace(/\/+$/, "").lastIndexOf("/");
  return i <= 0 ? "/" : path.slice(0, i);
}

export function baseName(path) {
  return path.split("/").pop();
}

export function formatSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

export function formatNumber(n) {
  return n.toLocaleString("en-US");
}

export function formatDuration(minutes) {
  if (minutes < 60) return `≈ ${minutes} min`;
  return `≈ ${Math.round(minutes / 60)} h`;
}

/** "1 book", "3 books": plural(n, "book", "books") */
export function plural(n, one, many) {
  return n === 1 ? one : many;
}

/** "3 books" */
export function count(n, one, many = one + "s") {
  return `${n.toLocaleString("en-US")} ${plural(n, one, many)}`;
}
