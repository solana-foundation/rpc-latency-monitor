#!/usr/bin/env node
import { createHmac } from "node:crypto";

const [sub, days = "365"] = process.argv.slice(2);
const secret = process.env.RAW_API_JWT_SECRET;

if (!sub || !secret) {
  console.error(
    "usage: RAW_API_JWT_SECRET=... scripts/mint-raw-api-token.mjs <partner> [days]",
  );
  process.exit(1);
}

const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
const now = Math.floor(Date.now() / 1000);
const body = `${encode({ alg: "HS256", typ: "JWT" })}.${encode({
  sub,
  iat: now,
  exp: now + Number(days) * 86400,
})}`;

console.log(`${body}.${createHmac("sha256", secret).update(body).digest("base64url")}`);
