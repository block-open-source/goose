import type { ReactNode } from "react";
import Link from "@docusaurus/Link";
import useDocusaurusContext from "@docusaurus/useDocusaurusContext";
import Layout from "@theme/Layout";

import styles from "./index.module.css";
import { GooseLogo } from "../components/GooseLogo";

function HeroSection() {
  return (
    <header className={styles.hero}>
      <div className={styles.heroInner}>
        <div className={styles.heroBadge}>
          Open Source · Apache 2.0 · Agentic AI Foundation
        </div>
        <div className={styles.heroLogo}>
          <GooseLogo />
        </div>
        <p className={styles.heroSubtitle}>
          Your native open source AI agent. Desktop app, CLI, and API — for code,
          workflows, and everything in between.
        </p>
        <div className={styles.heroActions}>
          <Link
            className="button button--primary button--lg"
            to="docs/getting-started/installation"
          >
            Install goose
          </Link>
          <Link
            className={`button button--outline button--lg ${styles.secondaryButton}`}
            to="docs/quickstart"
          >
            Quickstart
          </Link>
        </div>
        <div className={styles.heroStats}>
          <div className={styles.stat}>
            <span className={styles.statNumber}>45k+</span>
            <span className={styles.statLabel}>GitHub stars</span>
          </div>
          <div className={styles.statDivider} />
          <div className={styles.stat}>
            <span className={styles.statNumber}>500+</span>
            <span className={styles.statLabel}>Contributors</span>
          </div>
          <div className={styles.statDivider} />
          <div className={styles.stat}>
            <span className={styles.statNumber}>70+</span>
            <span className={styles.statLabel}>MCP extensions</span>
          </div>
        </div>
      </div>
    </header>
  );
}

type FeatureCardProps = {
  title: string;
  description: ReactNode;
  icon: string;
};

function FeatureCard({ title, description, icon }: FeatureCardProps) {
  return (
    <div className={styles.featureCard}>
      <div className={styles.featureIcon}>{icon}</div>
      <h3 className={styles.featureTitle}>{title}</h3>
      <div className={styles.featureDescription}>{description}</div>
    </div>
  );
}

type SmallCardProps = {
  title: string;
  description: ReactNode;
  icon: string;
};

function SmallCard({ title, description, icon }: SmallCardProps) {
  return (
    <div className={styles.smallCard}>
      <div className={styles.smallCardIcon}>{icon}</div>
      <h3 className={styles.smallCardTitle}>{title}</h3>
      <div className={styles.smallCardDescription}>{description}</div>
    </div>
  );
}

function FeaturesSection() {
  return (
    <section className={styles.section}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>What goose does</h2>
        <p className={styles.sectionSubtitle}>
          goose is a general-purpose AI agent that runs on your machine. Not
          just for code — use it for research, writing, automation, data
          analysis, or anything you need to get done.
        </p>
        <div className={styles.featuresGridTop}>
          <FeatureCard
            icon="🖥️"
            title="Desktop app, CLI, and API"
            description={
              <p>
                A native desktop app for macOS, Linux, and Windows. A full CLI
                for terminal workflows. An API to embed it anywhere. Built
                in Rust for performance and portability.
              </p>
            }
          />
          <FeatureCard
            icon="🔌"
            title="Extensible"
            description={
              <p>
                Connect to 70+ extensions — databases, APIs, browsers, GitHub,
                Google Drive, and more — via the{" "}
                <a href="https://modelcontextprotocol.io/" target="_blank" rel="noopener">
                  Model Context Protocol
                </a>{" "}
                open standard. Add{" "}
                <Link to="/docs/guides/context-engineering/using-skills">skills</Link>,
                {" "}or{" "}
                <Link to="/docs/tutorials/custom-extensions">build your own extension</Link>.
              </p>
            }
          />
          <FeatureCard
            icon="🤖"
            title="Any LLM, including your subscriptions"
            description={
              <p>
                Works with 15+ providers — Anthropic, OpenAI, Google, Ollama,
                OpenRouter, Azure, Bedrock, and more. Use API keys or your
                existing Claude, ChatGPT, or Gemini subscriptions via{" "}
                <Link to="/docs/guides/acp-providers">ACP</Link>.
              </p>
            }
          />
        </div>
        <div className={styles.featuresGridBottom}>
          <SmallCard
            icon="📋"
            title="Recipes"
            description={
              <p>
                Capture workflows as portable YAML configs. Share with your
                team, run in CI, include instructions, extensions, parameters,
                and{" "}
                <Link to="/docs/guides/recipes/session-recipes">subrecipes</Link>.
              </p>
            }
          />
          <SmallCard
            icon="🧩"
            title="MCP Apps"
            description={
              <p>
                Extensions can render interactive UIs directly inside goose
                Desktop — buttons, forms, visualizations. A new way to build{" "}
                <Link to="/docs/tutorials/building-mcp-apps">
                  agent-powered tools
                </Link>.
              </p>
            }
          />
          <SmallCard
            icon="🔀"
            title="Subagents"
            description={
              <p>
                Spawn independent{" "}
                <Link to="/docs/guides/context-engineering/subagents">subagents</Link> to handle
                tasks in parallel — code review, research, file processing —
                keeping the main conversation clean.
              </p>
            }
          />
          <SmallCard
            icon="🔒"
            title="Security"
            description={
              <p>
                Prompt injection detection, tool permission controls, sandbox
                mode, and an{" "}
                <Link to="/docs/guides/security/adversary-mode">
                  adversary reviewer
                </Link>{" "}
                that watches for unsafe actions.
              </p>
            }
          />
        </div>
      </div>
    </section>
  );
}

function StandardsSection() {
  return (
    <section className={`${styles.section} ${styles.sectionAlt}`}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>Built on open standards</h2>
        <div className={styles.standardsGrid}>
          <div className={styles.standardCard}>
            <h3>Model Context Protocol</h3>
            <p>
              <a href="https://modelcontextprotocol.io/" target="_blank" rel="noopener">MCP</a>{" "}
              is the open standard for connecting AI agents to tools and data
              sources. goose was one of the earliest adopters and has one of the
              deepest integrations in the ecosystem — with 70+ documented
              extensions and growing.
            </p>
            <Link to="/docs/category/mcp-servers">Browse MCP extensions →</Link>
          </div>
          <div className={styles.standardCard}>
            <h3>Agent Client Protocol</h3>
            <p>
              <a href="https://agentclientprotocol.com/" target="_blank" rel="noopener">ACP</a>{" "}
              is a standard for communicating with coding agents. goose works as
              an ACP server — connect from Zed, JetBrains, or VS Code — and can
              use ACP agents like Claude Code and Codex as providers.
            </p>
            <Link to="/docs/gdk/acp">goose as ACP server →</Link>
          </div>
          <div className={styles.standardCard}>
            <h3>Agentic AI Foundation</h3>
            <p>
              goose is part of the{" "}
              <a href="https://aaif.io/" target="_blank" rel="noopener">
                Agentic AI Foundation
              </a>{" "}
              at the Linux Foundation — ensuring the project remains
              vendor-neutral, community-governed, and open for the long term.
            </p>
            <a href="https://aaif.io/" target="_blank" rel="noopener">
              Learn about AAIF →
            </a>
          </div>
        </div>
      </div>
    </section>
  );
}

function CommunitySection() {
  return (
    <section className={styles.section}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>Community</h2>
        <p className={styles.sectionSubtitle}>
          An active community of developers, contributors, and users building
          extensions, sharing recipes, and pushing the boundaries of what local
          AI agents can do.
        </p>
        <div className={styles.communityGrid}>
          <a
            href="https://discord.gg/n8R5VaWDAn"
            target="_blank"
            rel="noopener"
            className={styles.communityCard}
          >
            <h3>💬 Discord</h3>
            <p>
              Ask questions, share what you've built, get help from the
              community.
            </p>
          </a>
          <a
            href="https://github.com/aaif-goose/goose"
            target="_blank"
            rel="noopener"
            className={styles.communityCard}
          >
            <h3>🐙 GitHub</h3>
            <p>
              Star, fork, file issues, contribute code. goose is built in the
              open.
            </p>
          </a>
          <Link to="/extensions" className={styles.communityCard}>
            <h3>🧩 Extensions</h3>
            <p>Browse community-built MCP extensions and add your own.</p>
          </Link>
          <Link to="/blog" className={styles.communityCard}>
            <h3>📝 Blog</h3>
            <p>Tutorials, deep dives, release notes, and community spotlights.</p>
          </Link>
        </div>
      </div>
    </section>
  );
}

function InstallSection() {
  return (
    <section className={`${styles.section} ${styles.sectionAlt}`}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>Get started</h2>
        <div className={styles.installBlock}>
          <div className={styles.installDesktop}>
            <Link
              className="button button--primary button--lg"
              to="docs/getting-started/installation"
            >
              Download the desktop app
            </Link>
            <p className={styles.installPlatforms}>
              Available for macOS, Linux, and Windows
            </p>
          </div>
          <div className={styles.installDivider}>
            <span>or install the CLI</span>
          </div>
          <div className={styles.installTerminal}>
            <div className={styles.terminalBar}>
              <span className={styles.terminalDot} />
              <span className={styles.terminalDot} />
              <span className={styles.terminalDot} />
            </div>
            <pre className={styles.terminalBody}>
              <code>
{`curl -fsSL https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | bash`}
              </code>
            </pre>
          </div>
        </div>
      </div>
    </section>
  );
}

function VideoSection() {
  return (
    <section className={styles.section}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>See goose in action</h2>
        <div className={styles.videoWrapper}>
          <iframe
            src="https://www.youtube.com/embed/D-DpDunrbpo"
            className={styles.video}
            title="vibe coding with goose"
            allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture"
            allowFullScreen
          />
        </div>
      </div>
    </section>
  );
}

export default function Home(): ReactNode {
  return (
    <Layout description="Your native open source AI agent. Desktop app, CLI, and API — for code, workflows, and everything in between.">
      <HeroSection />
      <main>
        <FeaturesSection />
        <StandardsSection />
        <CommunitySection />
        <InstallSection />
        <VideoSection />
      </main>
    </Layout>
  );
}
