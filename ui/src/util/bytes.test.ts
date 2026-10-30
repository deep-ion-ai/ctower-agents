import { humanBytes } from "./bytes";

test("bytes como o ctower clean", () => {
  expect(humanBytes(0)).toBe("0 B");
  expect(humanBytes(512)).toBe("512 B");
  expect(humanBytes(1536)).toBe("1,5 KB");
  expect(humanBytes(7_086_696_038)).toBe("6,6 GB");
});
