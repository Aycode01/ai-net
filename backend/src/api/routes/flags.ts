/**
 * Feature-flag admin endpoints (#425).
 *
 * GET  /api/admin/flags        — list all flags with state + source
 * PUT  /api/admin/flags/:flag  — set runtime override { "enabled": true|false }
 * DELETE /api/admin/flags/:flag — clear runtime override (fall back to env/default)
 */
import { Router, Request, Response } from "express";
import {
  getAllFlags,
  setFlag,
  KNOWN_FLAGS,
  type FeatureFlag,
} from "../../services/featureFlags";
import { adminAuthMiddleware } from "../middleware/auth";
import { NotFoundError, ValidationError } from "../../errors";

export function createFlagsRouter(): Router {
  const router = Router();

  router.use(adminAuthMiddleware);

  /**
   * @openapi
   * /api/admin/flags:
   *   get:
   *     summary: List all feature flags
   *     operationId: listFeatureFlags
   *     description: Returns every known feature flag with its effective value and the source that decided it (`runtime`, `env` or `default`).
   *     tags: [Admin, FeatureFlags]
   *     security:
   *       - adminApiKey: []
   *     responses:
   *       200:
   *         description: Feature flag states
   *         content:
   *           application/json:
   *             schema:
   *               type: object
   *               properties:
   *                 flags:
   *                   type: object
   *                   additionalProperties:
   *                     $ref: '#/components/schemas/FeatureFlagState'
   *       401:
   *         description: Missing or invalid admin API key
   *       503:
   *         description: ADMIN_API_KEY is not configured
   */
  router.get("/", (_req: Request, res: Response) => {
    res.json({ flags: getAllFlags() });
  });

  /**
   * @openapi
   * /api/admin/flags/{flag}:
   *   put:
   *     summary: Set a feature flag override
   *     operationId: setFeatureFlag
   *     description: Applies a runtime override for a known flag, taking precedence over the environment value until the process restarts.
   *     tags: [Admin, FeatureFlags]
   *     security:
   *       - adminApiKey: []
   *     parameters:
   *       - in: path
   *         name: flag
   *         required: true
   *         schema: { type: string }
   *         description: Feature flag name
   *         example: streaming_responses
   *     requestBody:
   *       required: true
   *       content:
   *         application/json:
   *           schema:
   *             type: object
   *             required: [enabled]
   *             properties:
   *               enabled: { type: boolean }
   *     responses:
   *       200:
   *         description: Override applied
   *         content:
   *           application/json:
   *             schema:
   *               $ref: '#/components/schemas/FeatureFlagState'
   *       400:
   *         description: 'Body must be a JSON object of the form { "enabled": true|false }'
   *         content:
   *           application/json:
   *             schema:
   *               $ref: '#/components/schemas/ValidationError'
   *       404:
   *         description: Unknown feature flag
   *         content:
   *           application/json:
   *             schema:
   *               $ref: '#/components/schemas/NotFoundError'
   *   delete:
   *     summary: Clear a feature flag override
   *     operationId: clearFeatureFlag
   *     description: Removes the runtime override, reverting the flag to its environment or default value.
   *     tags: [Admin, FeatureFlags]
   *     security:
   *       - adminApiKey: []
   *     parameters:
   *       - in: path
   *         name: flag
   *         required: true
   *         schema: { type: string }
   *         description: Feature flag name
   *         example: streaming_responses
   *     responses:
   *       200:
   *         description: Override cleared
   *         content:
   *           application/json:
   *             schema:
   *               $ref: '#/components/schemas/FeatureFlagState'
   *       404:
   *         description: Unknown feature flag
   *         content:
   *           application/json:
   *             schema:
   *               $ref: '#/components/schemas/NotFoundError'
   */
  router.put("/:flag", (req: Request, res: Response) => {
    const flag = req.params.flag as FeatureFlag;

    if (!(KNOWN_FLAGS as readonly string[]).includes(flag)) {
      throw new NotFoundError("Feature flag", flag);
    }

    const { enabled } = req.body as { enabled?: unknown };
    if (typeof enabled !== "boolean") {
      throw new ValidationError('Body must include { "enabled": true | false }', {
        field: "enabled",
        received: typeof enabled,
      });
    }

    setFlag(flag, enabled);
    res.json({ flag, enabled, source: "runtime" });
  });

  router.delete("/:flag", (req: Request, res: Response) => {
    const flag = req.params.flag as FeatureFlag;

    if (!(KNOWN_FLAGS as readonly string[]).includes(flag)) {
      throw new NotFoundError("Feature flag", flag);
    }

    setFlag(flag, null);
    const all = getAllFlags();
    res.json({ flag, ...all[flag] });
  });

  return router;
}
