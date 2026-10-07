from PIL import Image, ImageDraw
S=1024
def draw():
    im=Image.new('RGBA',(S,S),(0,0,0,0)); d=ImageDraw.Draw(im)
    # window
    d.rounded_rectangle((40,90,984,934),radius=110,fill=(52,58,74,255))
    d.rounded_rectangle((40,260,984,934),radius=110,fill=(30,34,44,255),corners=(False,False,True,True))
    for i,c in enumerate([(255,95,86),(255,189,46),(39,201,63)]):
        x=140+i*95; d.ellipse((x-32,175-32,x+32,175+32),fill=c+(255,))
    # folder
    d.rounded_rectangle((150,330,470,420),radius=30,fill=(230,160,30,255))
    d.rounded_rectangle((150,380,874,800),radius=50,fill=(255,196,61,255))
    d.rounded_rectangle((150,430,874,800),radius=50,fill=(255,210,90,255))
    # prompt >_
    w=56
    d.line((300,520,420,615),fill=(30,34,44,255),width=w); d.line((420,615,300,710),fill=(30,34,44,255),width=w)
    for x,y in((300,520),(420,615),(300,710)): d.ellipse((x-w//2,y-w//2,x+w//2,y+w//2),fill=(30,34,44,255))
    d.rounded_rectangle((480,680,700,736),radius=20,fill=(30,34,44,255))
    return im
im=draw()
im.save('icon.png')
im.save('file-browser.ico',sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
