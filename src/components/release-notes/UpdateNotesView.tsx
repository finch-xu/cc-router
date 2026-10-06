import { useMemo } from "react";
import { useT } from "@/i18n";
import { type MdListItem, parseMarkdown, pickNotesLang } from "@/lib/updateNotesMarkdown";
import { Inlines } from "./NotesDocView";

/** 与 NotesDocView 一致: 分节小签按出现顺序轮换颜色 */
const CHIP_CLASSES = ["rn-chip-a", "rn-chip-b", "rn-chip-c"] as const;

function ListItems({ items }: { items: MdListItem[] }) {
  return (
    <>
      {items.map((item, i) => (
        <li key={i}>
          <Inlines parts={item.text} />
          {item.children.length > 0 && (
            <ul className="rn-ul">
              <ListItems items={item.children} />
            </ul>
          )}
        </li>
      ))}
    </>
  );
}

/** 检查更新页的新版本内容: 只取界面语言那一段, 按 Markdown 渲染; 样式沿用更新内容弹窗的 rn-* */
export function UpdateNotesView({ body }: { body: string }) {
  const { locale } = useT();
  const blocks = useMemo(() => parseMarkdown(pickNotesLang(body, locale)), [body, locale]);

  let chips = 0;

  return (
    <div className="update-notes">
      {blocks.map((b, i) => {
        switch (b.kind) {
          case "heading":
            // 一、二级标题画成分节小签; 更深的标题只加粗
            return (
              <h3
                key={i}
                className={
                  b.level <= 2 ? `rn-chip ${CHIP_CLASSES[chips++ % CHIP_CLASSES.length]}` : "un-h"
                }
              >
                <Inlines parts={b.text} />
              </h3>
            );
          case "paragraph":
            return (
              <p key={i} className="rn-sum">
                <Inlines parts={b.text} />
              </p>
            );
          case "list": {
            const Tag = b.ordered ? "ol" : "ul";
            return (
              <Tag key={i} className="rn-ul">
                <ListItems items={b.items} />
              </Tag>
            );
          }
          case "code":
            return (
              <pre key={i} className="un-code">
                {b.text}
              </pre>
            );
          case "rule":
            return <hr key={i} className="un-rule" />;
        }
      })}
    </div>
  );
}
