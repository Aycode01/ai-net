import fs from "fs";
import path from "path";
import { loadConfig, resetConfigForTests, envSchema } from "../src/config";

describe("config validation & schema parity", () => {
  afterEach(() => {
    resetConfigForTests();
  });

  it("reports invalid environment variables by name", () => {
    let message = "";
    try {
      loadConfig({
        ...process.env,
        NODE_ENV: "production",
        PORT: "not-a-port",
        DATABASE_URL: "",
        VENICE_API_KEY: "",
      });
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    }

    expect(message).toContain("PORT");
    expect(message).toContain("DATABASE_URL");
    expect(message).toContain("VENICE_API_KEY");
  });

  it(".env.example and envSchema shape agree in both directions", () => {
    const envExamplePath = path.resolve(__dirname, "../.env.example");
    const content = fs.readFileSync(envExamplePath, "utf8");

    // Extract all KEY= or # KEY= declarations from .env.example
    const envKeys = new Set<string>();
    const lines = content.split("\n");
    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      const match = trimmed.match(/^(?:#\s*)?([A-Z0-9_]+)=/);
      if (match) {
        envKeys.add(match[1]);
      }
    }

    const schemaKeys = new Set(Object.keys(envSchema.shape));

    const missingInEnvExample = [...schemaKeys].filter((k) => !envKeys.has(k));
    const missingInSchema = [...envKeys].filter((k) => !schemaKeys.has(k));

    expect(missingInEnvExample).toEqual([]);
    expect(missingInSchema).toEqual([]);
  });
});
