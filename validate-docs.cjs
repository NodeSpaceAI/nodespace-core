const fs = require('fs'); 
const path = require('path');

function findMarkdownFiles(dir) {
    const files = []; 
    if (!fs.existsSync(dir)) return files;
    const items = fs.readdirSync(dir);
    for (const item of items) {
        if (item === 'node_modules' || item === '.git') continue;
        const fullPath = path.join(dir, item);
        const stat = fs.statSync(fullPath);
        if (stat.isDirectory()) { 
            files.push(...findMarkdownFiles(fullPath)); 
        }
        else if (item.endsWith('.md')) { 
            files.push(fullPath); 
        }
    } 
    return files;
}

const markdownFiles = findMarkdownFiles('.'); 
let brokenLinks = 0;

for (const file of markdownFiles) {
    const content = fs.readFileSync(file, 'utf8');
    const links = content.match(/\[([^\]]+)\]\(([^)]+)\)/g) || [];
    for (const link of links) {
        const match = link.match(/\[([^\]]+)\]\(([^)]+)\)/);
        if (match && !match[2].startsWith('http')) {
            const linkPath = match[2]; 
            const basePath = path.dirname(file);
            const resolvedPath = path.resolve(basePath, linkPath);
            if (!fs.existsSync(resolvedPath)) {
                console.log(`❌ ${file}: Broken link "${linkPath}"`); 
                brokenLinks++;
            }
        }
    }
}

if (brokenLinks === 0) { 
    console.log('✅ All documentation links validated'); 
}
else { 
    console.log(`❌ Found ${brokenLinks} broken links`); 
    process.exit(1); 
}