import org.scilab.forge.jlatexmath.*;

public class Bench {
    static final String[] NAMES = {"简单变量","上标","上下标","希腊字母","分式","嵌套分式","根号","n 次根","二次方程","求和","积分","矩阵","括号自适应","重音","长公式"};
    static final String[] CASES = {
        "x", "x^2", "a_{i}^{2}", "\\alpha + \\beta = \\gamma", "\\frac{a}{b}",
        "\\frac{1}{1+\\frac{1}{1+\\frac{1}{x}}}", "\\sqrt{x}", "\\sqrt[3]{x+1}",
        "x = \\frac{-b\\pm\\sqrt{b^2-4ac}}{2a}", "\\sum_{i=1}^{n} i^2", "\\int_0^1 x^2 \\, dx",
        "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}", "\\left( \\frac{a}{b} \\right)",
        "\\hat{x} + \\bar{y}", "e^{i\\pi} + 1 = 0 \\quad \\forall x \\in \\mathbb{R}"
    };

    static double pct(double[] a, double p) {
        int i = (int) Math.round((a.length - 1) * p);
        return a[i];
    }

    public static void main(String[] args) throws Exception {
        int iters = args.length > 0 ? Integer.parseInt(args[0]) : 2000;
        TeXFont font = new DefaultTeXFont(10f);
        System.out.printf("%-14s %10s %10s%n", "用例", "中位(µs)", "p95(µs)");
        System.out.println("------------------------------------");
        int ok = 0, fail = 0;
        for (int k = 0; k < CASES.length; k++) {
            String src = CASES[k];
            try {
                for (int i = 0; i < 500; i++) new TeXFormula(src).createTeXIcon(TeXConstants.STYLE_DISPLAY, 10f);
                double[] s = new double[iters];
                for (int i = 0; i < iters; i++) {
                    long t0 = System.nanoTime();
                    TeXIcon b = new TeXFormula(src).createTeXIcon(TeXConstants.STYLE_DISPLAY, 10f);
                    s[i] = (System.nanoTime() - t0) / 1000.0;
                }
                java.util.Arrays.sort(s);
                System.out.printf("%-14s %10.2f %10.2f%n", NAMES[k], pct(s, 0.5), pct(s, 0.95));
                ok++;
            } catch (Throwable e) {
                System.out.printf("%-14s   解析失败: %s%n", NAMES[k], e.getClass().getSimpleName());
                fail++;
            }
        }
        System.out.println("------------------------------------");
        System.out.println("成功 " + ok + " 条，失败 " + fail + " 条");
    }
}
