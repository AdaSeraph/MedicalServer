"""Pure Python 3.15 stdlib fixture generators for MedicalServer E2E tests.

Generates deterministic OOXML (.xlsx, .docx), PDF 1.4 (text-layer & scanned empty),
large >100k JSONL files, and 5,000-row patient CSVs using zero external packages.
"""

from __future__ import annotations

import csv
import json
import xml.sax.saxutils as saxutils
import zipfile
from pathlib import Path
from typing import Iterable, Sequence


def generate_large_jsonl(path: Path | str, count: int = 100_005) -> Path:
    """Write `count` valid KnowledgeDocument JSON lines to `path`."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", encoding="utf-8", newline="\n") as fh:
        for i in range(1, count + 1):
            doc = {
                "doc_id": f"gen_doc_{i:06d}",
                "source_title": "内科学（第9版）",
                "source_type": "textbook",
                "authority_level": 1.0 if i % 2 == 0 else 0.9,
                "disease_name": "流行性感冒" if i % 3 == 0 else "急性支气管炎",
                "department": "呼吸内科",
                "target_population": ["adult", "elderly"],
                "section_type": "clinical_manifestation",
                "content": (
                    f"第{i}条标准医学知识切片：表现为发热、咽痛、咳嗽及乏力症状，"
                    "建议结合临床体征与病程综合分诊评估。"
                ),
                "key_symptoms": ["发热", "咳嗽", "咽痛"],
                "seasonal_tags": ["winter", "influenza_season"],
            }
            fh.write(json.dumps(doc, ensure_ascii=False))
            fh.write("\n")
    return out


def generate_minimal_docx(
    path: Path | str,
    sections: Sequence[tuple[str, str]] | None = None,
) -> Path:
    """Generate a valid OOXML .docx archive using stdlib zipfile + XML."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    if sections is None:
        sections = [
            ("急性支气管炎", "Heading1"),
            ("临床表现", "Heading2"),
            (
                "起病较急，常先有上呼吸道感染症状，随后出现刺激性干咳或咳少量黏液痰，"
                "伴发热、咽痛及胸骨后不适感，体温多在38℃左右，持续3~5天逐步缓解。",
                "Normal",
            ),
            ("推荐科室", "Heading2"),
            ("建议首诊呼吸内科；若伴持续高热或呼吸急促，可前往发热门诊或急诊科。", "Normal"),
        ]

    content_types = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/word/document.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        "</Types>"
    )
    rels = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" '
        'Target="word/document.xml"/>'
        "</Relationships>"
    )

    para_xml_parts: list[str] = []
    for text, style in sections:
        escaped = saxutils.escape(text)
        p_pr = ""
        if style and style != "Normal":
            esc_style = saxutils.escape(style, {'"': "&quot;"})
            p_pr = f'<w:pPr><w:pStyle w:val="{esc_style}"/></w:pPr>'
        para_xml_parts.append(
            f"<w:p>{p_pr}<w:r><w:t>{escaped}</w:t></w:r></w:p>"
        )

    document_xml = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        f"<w:body>{''.join(para_xml_parts)}<w:sectPr/></w:body>"
        "</w:document>"
    )

    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("[Content_Types].xml", content_types.encode("utf-8"))
        zf.writestr("_rels/.rels", rels.encode("utf-8"))
        zf.writestr("word/document.xml", document_xml.encode("utf-8"))
    return out


def _col_letter(idx: int) -> str:
    """1-based column index -> Excel column letters (A, B, ..., AA)."""
    letters: list[str] = []
    n = idx
    while n > 0:
        n, rem = divmod(n - 1, 26)
        letters.append(chr(65 + rem))
    return "".join(reversed(letters))


def generate_minimal_xlsx(
    path: Path | str,
    headers: Sequence[str] | None = None,
    rows: Sequence[Sequence[object]] | None = None,
) -> Path:
    """Generate a valid OOXML .xlsx workbook readable by calamine using stdlib only."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    if headers is None:
        headers = [
            "姓名", "身份证号", "手机号", "详细住址", "年龄", "性别", "孕产状态",
            "慢病史", "过敏史", "手术史", "长期用药", "就诊时间", "主诉",
            "提取症状", "推荐科室", "紧急等级", "摘要",
        ]
    if rows is None:
        rows = [
            [
                "张伟", "110101199001011234", "13800138000", "北京市朝阳区幸福路10号3单元",
                34, "male", "false", "高血压", "青霉素", "无", "氨氯地平",
                "2026-10-01T10:00:00Z", "头晕伴血压升高", "头晕;血压升高",
                "心血管内科", 2, "既往高血压控制不佳建议心血管内科调整用药",
            ],
            [
                "李敏", "11010119850505122X", "13912345678", "上海市浦东新区张江路88号",
                41, "female", "false", "过敏性鼻炎;轻度哮喘", "海鲜", "阑尾切除术(2020)",
                "布地奈德吸入剂", "2026-10-02T14:20:00Z", "夜间干咳胸闷", "干咳;胸闷",
                "呼吸内科", 2, "换季诱发轻度哮喘急性发作风险建议呼吸内科评估",
            ],
        ]

    all_rows = [list(headers), *[list(r) for r in rows]]
    sheet_rows_xml: list[str] = []
    for r_idx, row in enumerate(all_rows, start=1):
        cells: list[str] = []
        for c_idx, val in enumerate(row, start=1):
            ref = f"{_col_letter(c_idx)}{r_idx}"
            if isinstance(val, (int, float)) and not isinstance(val, bool):
                cells.append(f'<c r="{ref}"><v>{val}</v></c>')
            else:
                text = saxutils.escape(str(val))
                cells.append(f'<c r="{ref}" t="inlineStr"><is><t>{text}</t></is></c>')
        sheet_rows_xml.append(f'<row r="{r_idx}">{"".join(cells)}</row>')

    content_types = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/xl/workbook.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        '<Override PartName="/xl/worksheets/sheet1.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        "</Types>"
    )
    root_rels = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" '
        'Target="xl/workbook.xml"/>'
        "</Relationships>"
    )
    workbook_xml = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
        '<sheets><sheet name="Patients" sheetId="1" r:id="rId1"/></sheets>'
        "</workbook>"
    )
    wb_rels = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" '
        'Target="worksheets/sheet1.xml"/>'
        "</Relationships>"
    )
    sheet_xml = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        f"<sheetData>{''.join(sheet_rows_xml)}</sheetData>"
        "</worksheet>"
    )

    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("[Content_Types].xml", content_types.encode("utf-8"))
        zf.writestr("_rels/.rels", root_rels.encode("utf-8"))
        zf.writestr("xl/workbook.xml", workbook_xml.encode("utf-8"))
        zf.writestr("xl/_rels/workbook.xml.rels", wb_rels.encode("utf-8"))
        zf.writestr("xl/worksheets/sheet1.xml", sheet_xml.encode("utf-8"))
    return out


def _pdf_escape(text: str) -> str:
    return text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")


def generate_minimal_pdf(
    path: Path | str,
    text_lines: Iterable[str] | None = None,
) -> Path:
    """Generate a valid PDF 1.4 file with an accurate xref table.

    If `text_lines` is None or empty, emits a 1-page PDF with 0 text operators
    (simulating an image-only scanned PDF to test OcrPlugin hooks).
    """
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    lines = list(text_lines) if text_lines else []

    if lines:
        ops = ["BT", "/F1 12 Tf", "72 740 Td"]
        for idx, line in enumerate(lines):
            if idx > 0:
                ops.append("0 -18 Td")
            ops.append(f"({_pdf_escape(line)}) Tj")
        ops.append("ET")
        stream_bytes = ("\n".join(ops) + "\n").encode("latin-1", errors="replace")
        font_obj = b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n"
        resources = b"<< /Font << /F1 5 0 R >> >>"
        objects = [
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
            (
                b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
                b"/Resources " + resources + b" /Contents 4 0 R >>\nendobj\n"
            ),
            (
                f"4 0 obj\n<< /Length {len(stream_bytes)} >>\nstream\n".encode("ascii")
                + stream_bytes
                + b"endstream\nendobj\n"
            ),
            font_obj,
        ]
    else:
        stream_bytes = b"% scanned image-only page with zero text operators\n"
        objects = [
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
            (
                b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
                b"/Resources << >> /Contents 4 0 R >>\nendobj\n"
            ),
            (
                f"4 0 obj\n<< /Length {len(stream_bytes)} >>\nstream\n".encode("ascii")
                + stream_bytes
                + b"endstream\nendobj\n"
            ),
        ]

    chunks: list[bytes] = [b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n"]
    offsets: list[int] = [0]
    cursor = len(chunks[0])
    for obj in objects:
        offsets.append(cursor)
        chunks.append(obj)
        cursor += len(obj)

    xref_offset = cursor
    xref_lines = [f"xref\n0 {len(objects) + 1}\n".encode("ascii"), b"0000000000 65535 f \n"]
    for off in offsets[1:]:
        xref_lines.append(f"{off:010d} 00000 n \n".encode("ascii"))
    trailer = (
        f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\n"
        f"startxref\n{xref_offset}\n%%EOF\n"
    ).encode("ascii")

    out.write_bytes(b"".join(chunks + xref_lines + [trailer]))
    return out


def generate_5000_patient_csv(path: Path | str, rows: int = 5000) -> Path:
    """Generate a deterministic `rows`-row synthetic patient CSV with PII for F10."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    headers = [
        "姓名", "身份证号", "手机号", "详细住址", "年龄", "性别", "孕产状态",
        "慢病史", "过敏史", "手术史", "长期用药", "就诊时间", "主诉",
        "提取症状", "推荐科室", "紧急等级", "摘要",
    ]
    with out.open("w", encoding="utf-8", newline="") as fh:
        writer = csv.writer(fh)
        writer.writerow(headers)
        for i in range(rows):
            age = (i % 90) + 1
            gender = "female" if i % 2 == 0 else "male"
            id_num = f"110101{1940 + (i % 60):04d}0101{1000 + (i % 8999):04d}"
            phone = f"138{10000000 + i:08d}"
            writer.writerow([
                f"测试患者{i:04d}",
                id_num,
                phone,
                f"北京市海淀区学院路{i % 200 + 1}号",
                age,
                gender,
                "false",
                "高血压" if i % 3 == 0 else "无",
                "青霉素" if i % 5 == 0 else "无",
                "无",
                "无",
                "2026-10-15T09:00:00Z",
                "发热咳嗽两天",
                "发热;咳嗽",
                "呼吸内科",
                2,
                "常规分诊摘要建议呼吸内科进一步评估。",
            ])
    return out
