import { Fragment } from "react";
import { runtime } from "@/runtime";
import type { NotesDoc, NotesInline } from "@/types";

/** 分节小签按出现顺序轮换颜色, 不按标题关键词 (三语通用, spec §3.2) */
const CHIP_CLASSES = ["rn-chip-a", "rn-chip-b", "rn-chip-c"] as const;

function Inline({ part }: { part: NotesInline }) {
  if (part.kind === "text") return <>{part.text}</>;
  if (part.kind === "bold") return <b>{part.text}</b>;
  if (part.kind === "code") return <code>{part.text}</code>;
  return (
    <a
      href={part.url}
      onClick={(e) => {
        e.preventDefault();
        runtime.openExternal(part.url).catch(() => {});
      }}
      // 中键点击不走 onClick, 也要拦住, 否则 webview 会自己开新窗口
      onAuxClick={(e) => e.preventDefault()}
    >
      {part.text}
    </a>
  );
}

export function Inlines({ parts }: { parts: NotesInline[] }) {
  return (
    <>
      {parts.map((p, i) => (
        <Fragment key={i}>
          <Inline part={p} />
        </Fragment>
      ))}
    </>
  );
}

/** 按 Rust 解析好的结构渲染一份说明; 全程不注入 HTML */
export function NotesDocView({ doc }: { doc: NotesDoc }) {
  return (
    <>
      {doc.summary.map((para, i) => (
        <p key={i} className="rn-sum">
          <Inlines parts={para} />
        </p>
      ))}
      {doc.sections.map((sec, si) => (
        <section key={si} className="rn-sec">
          <h3 className={`rn-chip ${CHIP_CLASSES[si % CHIP_CLASSES.length]}`}>{sec.heading}</h3>
          <ul className="rn-ul">
            {sec.items.map((item, ii) => (
              <li key={ii}>
                <Inlines parts={item.text} />
                {item.children.length > 0 && (
                  <ul className="rn-ul">
                    {item.children.map((child, ci) => (
                      <li key={ci}>
                        <Inlines parts={child} />
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
          </ul>
        </section>
      ))}
    </>
  );
}
