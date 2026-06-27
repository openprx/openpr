import { mkdirSync } from "node:fs";
import { expect, test, type Page, type TestInfo } from "@playwright/test";

const email = process.env.TEST_EMAIL ?? "demo@openpr.local";
const password = process.env.TEST_PASSWORD ?? "OpenPRDemo123!";
const workspaceId =
  process.env.OPENPR_WORKSPACE_ID ?? "07f6e023-6b0a-425c-bdac-442b5d36cd0c";
const projectId =
  process.env.OPENPR_PROJECT_ID ?? "8f7f7726-948e-4ea4-b149-06f25753b525";
const codeTaskFormId =
  process.env.OPENPR_CODE_TASK_FORM_ID ??
  "db4d73cc-9639-44b9-9875-21be69b0831f";
const artifactDir =
  "/opt/worker/task/openpr/test/artifacts/universal-forms-interaction-ia";

async function login(page: Page) {
  const response = await page.request.post("/api/v1/auth/login", {
    data: { email, password },
  });
  expect(response.ok()).toBe(true);
  const body = await response.json();
  expect(body.code).toBe(0);
  const { tokens, user } = body.data;

  await page.addInitScript(
    ({ accessToken, refreshToken, authUser }) => {
      localStorage.setItem("auth_token", accessToken);
      localStorage.setItem("refresh_token", refreshToken);
      localStorage.setItem("auth_user", JSON.stringify(authUser));
      localStorage.setItem("locale", "zh");
    },
    {
      accessToken: tokens.access_token,
      refreshToken: tokens.refresh_token,
      authUser: user,
    },
  );

  return tokens;
}

function formsPath() {
  return `/workspace/${workspaceId}/projects/${projectId}/forms?form=${codeTaskFormId}`;
}

function artifactPath(name: string) {
  mkdirSync(artifactDir, { recursive: true });
  return `${artifactDir}/${name}.png`;
}

async function attachScreenshot(page: Page, testInfo: TestInfo, name: string) {
  const path = artifactPath(name);
  await page.screenshot({ path, fullPage: true });
  await testInfo.attach(name, { path, contentType: "image/png" });
}

async function pageHasNoDocumentHorizontalOverflow(page: Page) {
  return page.evaluate(() => {
    const root = document.documentElement;
    const body = document.body;
    return (
      root.scrollWidth <= window.innerWidth + 1 &&
      body.scrollWidth <= window.innerWidth + 1
    );
  });
}

test.describe("Universal forms / interaction IA", () => {
  test.beforeEach(async ({ page }) => {
    await login(page);
    await page.goto(formsPath());
    await expect(page.locator("[data-form-data-list-toolbar]")).toBeVisible();
    await expect(page.locator("#form-switcher")).toHaveValue(codeTaskFormId);
  });

  test("selected form opens as a record list with drawer data entry", async ({
    page,
  }, testInfo) => {
    await expect(page.locator("[data-form-data-list-toolbar]")).toBeVisible();
    await expect(page.getByRole("heading", { name: "记录列表" })).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "数据表：Code Task" }),
    ).toHaveCount(0);
    await expect(page.getByText("筛选字段")).toHaveCount(0);
    await expect(page.locator("[data-record-editor-drawer]")).toHaveCount(0);
    await expect(page.locator('[data-form-mode="field-design"]')).toHaveCount(
      0,
    );

    await page.getByRole("button", { name: "新增", exact: true }).click();
    const drawer = page.locator("[data-record-editor-drawer]");
    await expect(drawer).toBeVisible();
    await expect(drawer.getByRole("heading", { name: "新建记录" })).toBeVisible();
    await drawer.getByRole("button", { name: "取消" }).click();
    await expect(drawer).toHaveCount(0);

    await page.getByRole("button", { name: "字段设计" }).click();
    await expect(page.locator("[data-form-config-workbench]")).toBeVisible();
    await expect(page.getByRole("button", { name: "录入数据" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "记录详情" })).toHaveCount(0);
    await expect(page.locator("[data-form-data-list-toolbar]")).toHaveCount(0);
    await expect(page.locator("[data-record-editor-drawer]")).toHaveCount(0);

    await attachScreenshot(page, testInfo, "record-list-drawer-config-split");
  });

  test("field design is isolated from detail layout and permission editing", async ({
    page,
  }, testInfo) => {
    await page.getByRole("button", { name: "字段设计" }).click();

    const workbench = page.locator('[data-form-mode="field-design"]');
    await expect(workbench).toBeVisible();
    await expect(workbench.locator("[data-field-designer-canvas]")).toBeVisible();
    await expect(
      page.getByText("这里只修改 Code Task 的字段结构"),
    ).toBeVisible();
    await expect(page.getByRole("heading", { name: "字段库" })).toBeVisible();
    await expect(page.getByRole("button", { name: "保存设计" })).toBeVisible();
    await expect(workbench.locator("[data-detail-header-builder]")).toHaveCount(
      0,
    );
    await expect(
      workbench.locator("[data-detail-layout-section-builder]"),
    ).toHaveCount(0);
    await expect(
      workbench.locator('[data-form-mode="permissions"]'),
    ).toHaveCount(0);
    await expect(page.getByRole("button", { name: "保存头部" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "保存权限" })).toHaveCount(0);
    const layout = await page.evaluate(() => {
      const library = document
        .querySelector('[data-form-mode="field-design"] [data-mobile-field-library-drawer]')
        ?.getBoundingClientRect();
      const canvas = document
        .querySelector('[data-form-mode="field-design"] [data-field-designer-canvas]')
        ?.getBoundingClientRect();
      const inspector = document
        .querySelector('[data-form-mode="field-design"] [data-mobile-field-inspector-drawer]')
        ?.getBoundingClientRect();
      return {
        topDelta:
          library && canvas && inspector
            ? Math.max(
                Math.abs(library.top - canvas.top),
                Math.abs(inspector.top - canvas.top),
              )
            : 999,
        ordered:
          library && canvas && inspector
            ? library.left < canvas.left && canvas.left < inspector.left
            : false,
        canvasWidth: canvas?.width ?? 0,
      };
    });
    expect(layout.topDelta).toBeLessThanOrEqual(1);
    expect(layout.ordered).toBe(true);
    expect(layout.canvasWidth).toBeGreaterThan(360);
    const buttonAudit = await page.evaluate(() => {
      const workbench = document.querySelector('[data-form-mode="field-design"]');
      if (!workbench) return { small: ['missing workbench'], libraryRows: 0, rowActions: 0 };
      const visibleButtons = [...workbench.querySelectorAll('button')].filter((button) => {
        const rect = button.getBoundingClientRect();
        const style = window.getComputedStyle(button);
        return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden';
      });
      const small = visibleButtons
        .map((button) => {
          const rect = button.getBoundingClientRect();
          return {
            label: button.getAttribute('aria-label') || button.textContent?.trim() || button.outerHTML,
            width: Math.round(rect.width),
            height: Math.round(rect.height),
          };
        })
        .filter((button) => button.width < 28 || button.height < 28)
        .map((button) => `${button.label}:${button.width}x${button.height}`);
      return {
        small,
        libraryRows: workbench.querySelectorAll('[data-field-library-type]').length,
        rowActions: [
          ...workbench.querySelectorAll(
            "[data-designer-field-move-up], [data-designer-field-move-down]",
          ),
        ].filter((button) => {
          const rect = button.getBoundingClientRect();
          return rect.width >= 32 && rect.height >= 32;
        }).length,
      };
    });
    expect(buttonAudit.small).toEqual([]);
    expect(buttonAudit.libraryRows).toBeGreaterThan(0);
    expect(buttonAudit.rowActions).toBeGreaterThan(0);

    await attachScreenshot(page, testInfo, "field-design-isolated-desktop");
  });

  test("field inspector keeps advanced properties collapsed until requested", async ({
    page,
  }, testInfo) => {
    await page.getByRole("button", { name: "字段设计" }).click();

    const workbench = page.locator('[data-form-mode="field-design"]');
    await expect(workbench).toBeVisible();
    await expect(workbench.getByText("字段名称")).toBeVisible();
    await expect(workbench.getByText("字段标识")).toBeVisible();
    await expect(
      workbench.locator('[data-field-inspector-section="input-behavior"]'),
    ).toBeVisible();
    await expect(
      workbench.locator('[data-field-inspector-section="validation"]'),
    ).toBeVisible();
    await expect(workbench.getByText("占位提示")).toBeHidden();
    await expect(workbench.getByText("正则")).toBeHidden();
    await expect(
      workbench.locator('[data-field-safety="selected"]'),
    ).toBeVisible();

    await attachScreenshot(
      page,
      testInfo,
      "field-inspector-progressive-disclosure-desktop",
    );
  });

  test("detail layout is a separate mode with its own builders", async ({
    page,
  }, testInfo) => {
    await page.getByRole("button", { name: "详情布局" }).click();

    const workbench = page.locator('[data-form-mode="detail-layout"]');
    await expect(workbench).toBeVisible();
    await expect(
      page.getByText("这里配置 Code Task 的记录详情页头部"),
    ).toBeVisible();
    await expect(
      workbench.locator("[data-detail-header-builder]"),
    ).toBeVisible();
    await expect(
      workbench.locator("[data-detail-highlights-builder]"),
    ).toBeHidden();
    await expect(
      workbench.locator("[data-detail-layout-section-builder]"),
    ).toBeHidden();
    await page.getByRole("button", { name: "详情高亮" }).click();
    await expect(
      workbench.locator("[data-detail-highlights-builder]"),
    ).toBeVisible();
    await page.getByRole("button", { name: "详情分区" }).click();
    await expect(
      workbench.locator("[data-detail-layout-section-builder]"),
    ).toBeVisible();
    await page.getByRole("button", { name: "详情页头部" }).click();
    await expect(page.getByRole("heading", { name: "字段库" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "保存设计" })).toHaveCount(0);

    await attachScreenshot(page, testInfo, "detail-layout-desktop");
  });

  test("permissions has a dedicated mode instead of living under automation", async ({
    page,
  }, testInfo) => {
    await page.getByRole("button", { name: "权限" }).click();

    const workbench = page.locator('[data-form-mode="permissions"]');
    await expect(workbench).toBeVisible();
    await expect(
      page.getByText("这里配置 Code Task 的成员动作、记录范围和字段读写策略"),
    ).toBeVisible();
    await expect(page.getByRole("heading", { name: "权限" })).toBeVisible();
    await expect(page.getByRole("button", { name: "保存权限" })).toBeVisible();
    await expect(page.locator("[data-permission-state-summary]")).toHaveCount(
      0,
    );
    await expect(page.locator('[data-form-mode="automation"]')).toHaveCount(0);

    await attachScreenshot(page, testInfo, "permissions-mode-desktop");
  });

  test("import export has a dedicated mode outside the data-entry first screen", async ({
    page,
  }, testInfo) => {
    await page.getByRole("button", { name: "导入导出" }).click();

    const workbench = page.locator('[data-form-mode="import-export"]');
    await expect(workbench).toBeVisible();
    await expect(page.getByText("这里处理 Code Task 的记录导入")).toBeVisible();
    await expect(page.getByRole("button", { name: "导入记录" })).toBeVisible();
    await expect(page.getByText("下载导入模板")).toBeVisible();
    await expect(
      page.getByRole("button", { name: "导出当前视图", exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("heading", { name: "新建记录" })).toHaveCount(
      0,
    );

    await attachScreenshot(page, testInfo, "import-export-mode-desktop");
  });

  test("expanded data-entry view tools keep advanced view parameters disclosed on demand", async ({
    page,
  }, testInfo) => {
    await page.getByRole("button", { name: "保存视图" }).click();

    const expandedTools = page.locator("[data-view-tools-expanded-panel]");
    await expect(expandedTools).toBeVisible();
    await expect(
      expandedTools.locator("[data-view-setup-disclosure]"),
    ).toBeVisible();
    await expect(
      expandedTools.locator("[data-view-columns-disclosure]"),
    ).toBeVisible();
    await expect(page.getByText("视图标识")).toBeHidden();
    await expect(
      expandedTools.locator("[data-view-columns-disclosure]"),
    ).toContainText("显示列");
    await expect(page.locator("[data-record-list-panel]")).toBeVisible();
    const widths = await page.evaluate(() => {
      const viewTools = document
        .querySelector("[data-view-tools-expanded-panel]")
        ?.getBoundingClientRect();
      const recordList = document
        .querySelector("[data-record-list-panel]")
        ?.getBoundingClientRect();
      return {
        leftDelta: viewTools && recordList ? Math.abs(viewTools.left - recordList.left) : 999,
        widthDelta: viewTools && recordList ? Math.abs(viewTools.width - recordList.width) : 999,
      };
    });
    expect(widths.leftDelta).toBeLessThanOrEqual(1);
    expect(widths.widthDelta).toBeLessThanOrEqual(1);

    await attachScreenshot(
      page,
      testInfo,
      "view-tools-disclosed-on-demand-desktop",
    );
  });

  test("import records uses a source mapping preview commit wizard", async ({
    page,
  }, testInfo) => {
    await page.getByRole("button", { name: "导入导出" }).click();
    await page.getByRole("button", { name: "导入记录" }).click();

    const wizard = page.locator("[data-import-wizard]");
    await expect(wizard).toBeVisible();
    await expect(wizard).toHaveAttribute("data-import-wizard-step", "source");
    await expect(page.locator('[data-import-step="source"]')).toHaveAttribute(
      "data-import-step-active",
      "true",
    );
    await expect(
      page.locator('[data-import-wizard-panel="source"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-import-wizard-panel="mapping"]'),
    ).toHaveCount(0);
    await attachScreenshot(page, testInfo, "import-wizard-source-desktop");

    await page
      .locator("#form-import-text")
      .fill("Title,Repository,CI status\nqa-wizard,repo,open\n");
    await page.getByRole("button", { name: "下一步" }).click();
    await expect(wizard).toHaveAttribute("data-import-wizard-step", "mapping");
    await expect(page.locator('[data-import-step="mapping"]')).toHaveAttribute(
      "data-import-step-active",
      "true",
    );
    await expect(page.locator("[data-import-mapping-wizard]")).toBeVisible();
    await expect(
      page.locator('[data-import-wizard-panel="source"]'),
    ).toHaveCount(0);
    await expect(
      page.locator('[data-import-wizard-panel="preview"]'),
    ).toHaveCount(0);
    await attachScreenshot(page, testInfo, "import-wizard-mapping-desktop");
  });

  test("mobile field design uses drawers for the field library and inspector", async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(formsPath());
    await page.getByRole("button", { name: "字段设计" }).click();

    const workbench = page.locator('[data-form-mode="field-design"]');
    await expect(workbench).toBeVisible();
    await expect(
      workbench.locator("[data-mobile-field-design-toolbar]"),
    ).toBeVisible();
    await expect(
      workbench.locator('[data-mobile-field-library-drawer="closed"]'),
    ).toBeHidden();
    await expect(
      workbench.locator('[data-mobile-field-inspector-drawer="closed"]'),
    ).toBeHidden();
    await expect(workbench.locator("[data-detail-header-builder]")).toHaveCount(
      0,
    );
    await expect(await pageHasNoDocumentHorizontalOverflow(page)).toBe(true);

    await attachScreenshot(page, testInfo, "field-design-mobile");

    await page.getByRole("button", { name: "字段库" }).click();
    await expect(
      workbench.locator('[data-mobile-field-library-drawer="open"]'),
    ).toBeVisible();
    await expect(page.getByRole("heading", { name: "字段库" })).toBeVisible();
    await expect(await pageHasNoDocumentHorizontalOverflow(page)).toBe(true);
    await attachScreenshot(page, testInfo, "field-design-mobile-library-drawer");
    await workbench
      .locator('[data-mobile-field-library-drawer="open"]')
      .getByRole("button", { name: "关闭", exact: true })
      .click();

    await page.getByRole("button", { name: "属性" }).click();
    await expect(
      workbench.locator('[data-mobile-field-inspector-drawer="open"]'),
    ).toBeVisible();
    await expect(workbench.getByText("字段名称")).toBeVisible();
    await expect(await pageHasNoDocumentHorizontalOverflow(page)).toBe(true);
    await attachScreenshot(
      page,
      testInfo,
      "field-design-mobile-inspector-drawer",
    );
  });
});

test.describe("Universal forms / project type entry", () => {
  test("opens custom form projects directly on the form list", async ({
    page,
  }, testInfo) => {
    const tokens = await login(page);
    const suffix = Date.now().toString().slice(-7);
    const projectName = `表单入口回归 ${suffix}`;
    const createResponse = await page.request.post(
      `/api/v1/workspaces/${workspaceId}/projects`,
      {
        headers: { Authorization: `Bearer ${tokens.access_token}` },
        data: {
          name: projectName,
          key: `F${suffix}`,
          description: "临时验证表单项目入口不会混入项目管理。",
          type_key: "custom_form",
        },
      },
    );
    expect(createResponse.ok()).toBe(true);
    const createBody = await createResponse.json();
    expect(createBody.code).toBe(0);
    const project = createBody.data as { id: string; name: string };
    const formName = `入口数据表 ${suffix}`;
    const formResponse = await page.request.post(
      `/api/v1/projects/${project.id}/forms`,
      {
        headers: { Authorization: `Bearer ${tokens.access_token}` },
        data: {
          key: `entry_${suffix}`,
          name: formName,
          description: "临时验证表单项目默认展示表单列表。",
          title_template: "{title}",
          schema: {
            version: "openpr.form.schema.v1",
            fields: [
              {
                key: "title",
                label: "标题",
                type: "text",
                required: true,
              },
            ],
          },
        },
      },
    );
    expect(formResponse.ok()).toBe(true);
    const formBody = await formResponse.json();
    expect(formBody.code).toBe(0);

    try {
      await page.goto(`/workspace/${workspaceId}/projects`);
      await page.getByRole("button", { name: project.name, exact: true }).click();
      await expect(page).toHaveURL(
        new RegExp(`/workspace/${workspaceId}/projects/${project.id}/forms$`),
      );
      await expect(page.getByRole("heading", { name: "万能表单" })).toBeVisible();
      await expect(page.getByRole("heading", { name: "表单列表" })).toBeVisible();
      await expect(page.locator("[data-form-list-home]")).toBeVisible();
      await expect(
        page.getByRole("heading", { name: `数据表：${formName}` }),
      ).toHaveCount(0);
      await expect(page.getByRole("button", { name: "新建表单" })).toBeVisible();
      await expect(
        page.getByRole("heading", { name: "项目管理" }),
      ).toHaveCount(0);

      await attachScreenshot(page, testInfo, "custom-form-project-entry");

      await page.getByRole("button", { name: new RegExp(formName) }).click();
      await expect(page).toHaveURL(
        new RegExp(`/workspace/${workspaceId}/projects/${project.id}/forms\\?form=`),
      );
      await expect(
        page.locator("[data-form-data-list-toolbar]"),
      ).toBeVisible();
      await expect(page.locator("#form-switcher")).toHaveValue(formBody.data.id);

      await page.goto(`/workspace/${workspaceId}/projects/${project.id}`);
      await expect(page).toHaveURL(
        new RegExp(`/workspace/${workspaceId}/projects/${project.id}/forms$`),
      );
      await expect(page.getByRole("heading", { name: "万能表单" })).toBeVisible();
      await expect(page.getByRole("heading", { name: "表单列表" })).toBeVisible();
      await expect(
        page.getByRole("heading", { name: "项目管理" }),
      ).toHaveCount(0);
    } finally {
      await page.request.delete(`/api/v1/projects/${project.id}`, {
        headers: { Authorization: `Bearer ${tokens.access_token}` },
      });
    }
  });
});
