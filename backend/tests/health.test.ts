import request from "supertest";
import { createApp } from "../src/api/app";

describe("Health Router", () => {
  let app: ReturnType<typeof createApp>["httpServer"];
  let closeApp: () => void;

  beforeAll(() => {
    const instance = createApp();
    app = instance.httpServer;
    closeApp = instance.close;
  });

  afterAll((done) => {
    closeApp();
    done();
  });

  it("GET /health returns 200 with status ok and version", async () => {
    const res = await request(app).get("/health");
    expect(res.status).toBe(200);
    expect(res.body.status).toBe("ok");
    expect(res.body).toHaveProperty("version");
    expect(res.body).toHaveProperty("uptime");
  });

  it("GET /health/live returns 200 with status ok", async () => {
    const res = await request(app).get("/health/live");
    expect(res.status).toBe(200);
    expect(res.body.status).toBe("ok");
  });

  it("GET /health/deep handles timeouts and returns status", async () => {
    const res = await request(app).get("/health/deep");
    expect([200, 503]).toContain(res.status);
    expect(res.body).toHaveProperty("services");
  });
});
